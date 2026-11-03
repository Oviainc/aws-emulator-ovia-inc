// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the AWS Transfer Family web app lifecycle.
//!
//! Every test drives the real wire path through the shared [`crate::harness`] — `POST /`
//! with `X-Amz-Target: TransferService.<Op>` — so what is asserted is what an SDK sees,
//! not what a handler returns.
//!
//! RULE under test: `CreateWebApp` mints a modelled `WebAppId`, persists the web app
//! with every member it accepts stored as given and every member the model documents a
//! default or derives from the resource resolved ONCE at create time
//! (`WebAppEndpointPolicy`, the `EndpointType` the endpoint configuration selects, the
//! web app's own endpoint, the default `AccessEndpoint`, the `VpcEndpointId` of a
//! VPC-hosted web app, and the Identity Center application the create registers), and
//! `DescribeWebApp` rebuilds all of them from that stored record.
//!
//! Each suite asserts that rule over a TABLE of inputs rather than one recorded call:
//! the rows differ along which members are present, which enum value each carries, how
//! many subnets/tags the list members hold, which form of Identity Center instance ARN
//! is named, and which Region the call is signed for.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::harness::{
    assert_members, is_modelled_vpc_endpoint_id, is_modelled_web_app_id, member_of, Emulator,
    ACCOUNT, REGION, ROLE,
};

/// An IAM Identity Center instance ARN satisfying `IdentityCenterInstanceArn`'s
/// modelled pattern `^arn:[\w-]+:sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}$`.
const INSTANCE_ARN: &str = "arn:aws:sso:::instance/ssoins-1234567890abcdef";

/// The `IdentityProviderDetails` union every create must send: `IdentityCenterConfig`
/// is the union's only member and the union itself is `@required`.
fn identity_center(config: Value) -> Value {
    json!({"IdentityCenterConfig": config})
}

/// The identity provider details a row that does not vary them sends.
fn default_identity_center() -> Value {
    identity_center(json!({"InstanceArn": INSTANCE_ARN, "Role": ROLE}))
}

/// Create one web app, assert the create succeeded with a modelled `WebAppId`, and
/// hand the id back.
async fn create_web_app(emulator: &Emulator, case: &str, request: Value) -> String {
    create_web_app_in(emulator, REGION, case, request).await
}

/// [`create_web_app`], signed for `region` so the handler's ARN and endpoint
/// derivation sees it.
async fn create_web_app_in(
    emulator: &Emulator,
    region: &str,
    case: &str,
    request: Value,
) -> String {
    let answer = emulator.call_in(region, "CreateWebApp", request).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateWebApp 200, got {} with body {}",
        answer.status, answer.body
    );
    let id = member_of(&answer.body, "WebAppId")
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateWebApp returned no WebAppId: {}", answer.body))
        .to_string();
    assert!(
        is_modelled_web_app_id(&id),
        "case {case:?}: expected a WebAppId matching ^webapp-[0-9a-f]{{17}}$ (len 24), got {id:?}"
    );
    id
}

/// Describe one web app and hand back the `WebApp` structure.
async fn describe_web_app(emulator: &Emulator, case: &str, web_app_id: &str) -> Value {
    let answer = emulator
        .call("DescribeWebApp", json!({"WebAppId": web_app_id}))
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected DescribeWebApp 200, got {} with body {}",
        answer.status, answer.body
    );
    member_of(&answer.body, "WebApp").clone()
}

/// The `IdentityCenterConfig` a description reports.
fn described_identity_center(web_app: &Value) -> &Value {
    member_of(
        member_of(web_app, "DescribedIdentityProviderDetails"),
        "IdentityCenterConfig",
    )
}

// ---------------------------------------------------------------------------
// CreateWebApp -> DescribeWebApp round trip, over the whole input class.
// ---------------------------------------------------------------------------

/// The members a row expects of its description, as a function of the minted id (the
/// derived endpoint and the default access endpoint carry it).
type Expect = fn(&str) -> Value;

/// RULE: every member `CreateWebApp` accepts is persisted as given, every member the
/// model documents a default for is resolved once at create time, and `DescribeWebApp`
/// rebuilds all of them from the stored web app — repeatably, because a description
/// changes nothing.
#[tokio::test]
async fn describe_web_app_rebuilds_every_created_member_from_stored_state() {
    let emulator = Emulator::new();

    let cases: Vec<(&str, Value, Expect)> = vec![
        (
            "all defaults: identity provider only",
            json!({"IdentityProviderDetails": default_identity_center()}),
            |id| {
                json!({
                    // The documented default: no custom URL was given, so the access
                    // endpoint is the `https://` URL of the web app's own endpoint.
                    "AccessEndpoint": format!("https://{id}.transfer-webapp.{REGION}.on.aws"),
                    "WebAppEndpointPolicy": "STANDARD",
                    "EndpointType": "PUBLIC",
                    // The model documents no `WebAppUnits` default, so none is invented.
                    "WebAppUnits": null,
                    // The tag set an untagged web app holds is reported EMPTY.
                    "Tags": [],
                    "DescribedEndpointDetails": null,
                })
            },
        ),
        (
            "custom access endpoint, FIPS policy, one provisioned unit, one tag",
            json!({
                "IdentityProviderDetails": default_identity_center(),
                "AccessEndpoint": "https://files.example.invalid",
                "WebAppEndpointPolicy": "FIPS",
                "WebAppUnits": {"Provisioned": 1},
                "Tags": [{"Key": "env", "Value": "dev"}],
            }),
            |_id| {
                json!({
                    "AccessEndpoint": "https://files.example.invalid",
                    "WebAppEndpointPolicy": "FIPS",
                    "EndpointType": "PUBLIC",
                    "WebAppUnits": {"Provisioned": 1},
                    "Tags": [{"Key": "env", "Value": "dev"}],
                    "DescribedEndpointDetails": null,
                })
            },
        ),
        (
            "explicit STANDARD policy, four provisioned units, three tags",
            json!({
                "IdentityProviderDetails": default_identity_center(),
                "WebAppEndpointPolicy": "STANDARD",
                "WebAppUnits": {"Provisioned": 4},
                "Tags": [
                    {"Key": "env", "Value": "prod"},
                    {"Key": "team", "Value": "files"},
                    {"Key": "cost-center", "Value": "42"},
                ],
            }),
            |id| {
                json!({
                    "AccessEndpoint": format!("https://{id}.transfer-webapp.{REGION}.on.aws"),
                    "WebAppEndpointPolicy": "STANDARD",
                    "EndpointType": "PUBLIC",
                    "WebAppUnits": {"Provisioned": 4},
                    "Tags": [
                        {"Key": "env", "Value": "prod"},
                        {"Key": "team", "Value": "files"},
                        {"Key": "cost-center", "Value": "42"},
                    ],
                })
            },
        ),
        (
            "a long access endpoint URL and a large unit count",
            json!({
                "IdentityProviderDetails": default_identity_center(),
                // `WebAppAccessEndpoint` is modelled `len 1..1024`.
                "AccessEndpoint": format!("https://{}.example.invalid", "a".repeat(200)),
                "WebAppUnits": {"Provisioned": 50},
            }),
            |_id| {
                json!({
                    "AccessEndpoint": format!("https://{}.example.invalid", "a".repeat(200)),
                    "WebAppUnits": {"Provisioned": 50},
                    "WebAppEndpointPolicy": "STANDARD",
                    "EndpointType": "PUBLIC",
                    "Tags": [],
                })
            },
        ),
        (
            "an identity provider naming only an instance",
            json!({
                "IdentityProviderDetails": identity_center(json!({"InstanceArn": INSTANCE_ARN})),
            }),
            |_id| json!({"EndpointType": "PUBLIC", "WebAppEndpointPolicy": "STANDARD"}),
        ),
        (
            "an identity provider naming only a role",
            json!({
                "IdentityProviderDetails": identity_center(json!({"Role": ROLE})),
            }),
            |_id| json!({"EndpointType": "PUBLIC", "WebAppEndpointPolicy": "STANDARD"}),
        ),
    ];

    let mut ids: BTreeSet<String> = BTreeSet::new();

    for (case, request, expect) in cases {
        let web_app_id = create_web_app(&emulator, case, request.clone()).await;
        assert!(
            ids.insert(web_app_id.clone()),
            "case {case:?}: expected a WebAppId no other create minted, got the repeat {web_app_id:?}"
        );

        let web_app = describe_web_app(&emulator, case, &web_app_id).await;
        assert_members(case, &web_app, &expect(&web_app_id));

        // The identity the create named is stored as given.
        let sent = member_of(
            member_of(&request, "IdentityProviderDetails"),
            "IdentityCenterConfig",
        );
        let described = described_identity_center(&web_app);
        for member in ["InstanceArn", "Role"] {
            assert_eq!(
                member_of(described, member),
                member_of(sent, member),
                "case {case:?}: expected IdentityCenterConfig.{member} to round trip as sent, \
                 got {:?}",
                member_of(described, member)
            );
        }

        // The resource's own identifiers, derived from the id and the signing Region.
        assert_members(
            case,
            &web_app,
            &json!({
                "WebAppId": web_app_id,
                "Arn": format!("arn:aws:transfer:{REGION}:{ACCOUNT}:webapp/{web_app_id}"),
                "WebAppEndpoint": format!("{web_app_id}.transfer-webapp.{REGION}.on.aws"),
            }),
        );

        // A description is a READ: a second one answers exactly the same thing.
        let again = describe_web_app(&emulator, case, &web_app_id).await;
        assert_eq!(
            again, web_app,
            "case {case:?}: expected a repeated DescribeWebApp to answer the same web app, \
             got {again:?} after {web_app:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The endpoint configuration selects the endpoint type.
// ---------------------------------------------------------------------------

/// RULE: a web app created with a VPC endpoint configuration is hosted on a `VPC`
/// endpoint and reports the VPC it was placed in plus the modelled `VpcEndpointId` of
/// the endpoint created for it; one created with no endpoint configuration is hosted on
/// a `PUBLIC` endpoint and has no VPC endpoint to report.
///
/// The rows differ along which members of the VPC configuration are present, how many
/// subnets and security groups they name, and which IP address type they ask for.
#[tokio::test]
async fn the_endpoint_configuration_selects_the_endpoint_type_and_mints_a_vpc_endpoint() {
    let emulator = Emulator::new();

    // `SecurityGroupIds` members are modelled `^sg-[0-9a-f]{8,17}$`, `len 11..20`;
    // `WebAppVpcEndpointIpAddressType` is `[IPV4, DUALSTACK]`.
    let vpc_cases: Vec<(&str, Value)> = vec![
        (
            "a full VPC configuration: vpc, two subnets, a security group, dualstack",
            json!({"Vpc": {
                "VpcId": "vpc-0123456789abcdef0",
                "SubnetIds": ["subnet-0123456789abcdef0", "subnet-00000000000000001"],
                "SecurityGroupIds": ["sg-0123456789abcdef0"],
                "IpAddressType": "DUALSTACK",
            }}),
        ),
        (
            "a VPC id alone",
            json!({"Vpc": {"VpcId": "vpc-00000000000000002"}}),
        ),
        (
            "one subnet and IPV4, no vpc id",
            json!({"Vpc": {
                "SubnetIds": ["subnet-0aaaaaaaaaaaaaaaa"],
                "IpAddressType": "IPV4",
            }}),
        ),
        (
            "three subnets and two security groups",
            json!({"Vpc": {
                "VpcId": "vpc-0123456789abcdef0",
                "SubnetIds": [
                    "subnet-0123456789abcdef0",
                    "subnet-0123456789abcdef1",
                    "subnet-0123456789abcdef2",
                ],
                "SecurityGroupIds": ["sg-0123456789abcdef0", "sg-abcdef012"],
            }}),
        ),
    ];

    let mut vpc_endpoint_ids: BTreeSet<String> = BTreeSet::new();

    for (case, endpoint_details) in vpc_cases {
        let web_app_id = create_web_app(
            &emulator,
            case,
            json!({
                "IdentityProviderDetails": default_identity_center(),
                "EndpointDetails": endpoint_details.clone(),
            }),
        )
        .await;
        let web_app = describe_web_app(&emulator, case, &web_app_id).await;

        assert_members(case, &web_app, &json!({"EndpointType": "VPC"}));

        let sent = member_of(&endpoint_details, "Vpc");
        let described = member_of(member_of(&web_app, "DescribedEndpointDetails"), "Vpc");
        for member in ["VpcId", "SubnetIds"] {
            assert_eq!(
                member_of(described, member),
                member_of(sent, member),
                "case {case:?}: expected Vpc.{member} to round trip as sent, got {:?}",
                member_of(described, member)
            );
        }
        // `DescribedWebAppVpcConfig` models neither member, so no reader reports one.
        for unmodelled in ["SecurityGroupIds", "IpAddressType"] {
            assert!(
                member_of(described, unmodelled).is_null(),
                "case {case:?}: DescribedWebAppVpcConfig models no {unmodelled}, got {:?}",
                member_of(described, unmodelled)
            );
        }

        let vpc_endpoint_id = member_of(described, "VpcEndpointId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            is_modelled_vpc_endpoint_id(&vpc_endpoint_id),
            "case {case:?}: expected a VpcEndpointId matching ^vpce-[0-9a-f]{{17}}$, \
             got {vpc_endpoint_id:?}"
        );
        assert!(
            vpc_endpoint_ids.insert(vpc_endpoint_id.clone()),
            "case {case:?}: expected this web app's own VPC endpoint, got the repeat \
             {vpc_endpoint_id:?}"
        );
    }

    // The counterpart: no endpoint configuration at all.
    let case = "no endpoint configuration";
    let public_id = create_web_app(
        &emulator,
        case,
        json!({"IdentityProviderDetails": default_identity_center()}),
    )
    .await;
    let public = describe_web_app(&emulator, case, &public_id).await;
    assert_members(
        case,
        &public,
        &json!({"EndpointType": "PUBLIC", "DescribedEndpointDetails": null}),
    );
}

// ---------------------------------------------------------------------------
// The Identity Center application the create registers.
// ---------------------------------------------------------------------------

/// RULE: a create that names an IAM Identity Center instance gets an application ARN
/// set automatically, inside THAT instance and unique to the web app; a create that
/// names no instance gets none, because there is no instance to register one in.
///
/// The rows differ along the form of instance id the modelled pattern
/// `(sso)?ins-[a-zA-Z0-9-.]{16}` admits, including one with the punctuation it allows,
/// and one row names no instance at all.
#[tokio::test]
async fn the_identity_center_application_arn_is_minted_inside_the_named_instance() {
    let emulator = Emulator::new();

    let cases: Vec<(&str, Value)> = vec![
        (
            "an ssoins- instance",
            json!({"InstanceArn": INSTANCE_ARN, "Role": ROLE}),
        ),
        (
            "an ins- instance",
            json!({"InstanceArn": "arn:aws:sso:::instance/ins-abcdef0123456789", "Role": ROLE}),
        ),
        (
            "an instance id carrying the dots and dashes the pattern allows",
            json!({"InstanceArn": "arn:aws:sso:::instance/ssoins-a.b-c.d-e.f-g1h2"}),
        ),
        ("no instance named", json!({"Role": ROLE})),
    ];

    let mut application_arns: BTreeSet<String> = BTreeSet::new();

    for (case, config) in cases {
        let web_app_id = create_web_app(
            &emulator,
            case,
            json!({"IdentityProviderDetails": identity_center(config.clone())}),
        )
        .await;
        let web_app = describe_web_app(&emulator, case, &web_app_id).await;
        let application = member_of(described_identity_center(&web_app), "ApplicationArn");

        let Some(instance_arn) = member_of(&config, "InstanceArn").as_str() else {
            assert!(
                application.is_null(),
                "case {case:?}: expected no ApplicationArn without an instance, got {application:?}"
            );
            continue;
        };

        let (_, instance) = instance_arn
            .split_once(":instance/")
            .expect("the probe instance ARN carries an instance segment");
        let prefix = format!("arn:aws:sso::{ACCOUNT}:application/{instance}/apl-");
        let application = application.as_str().unwrap_or_default().to_string();
        let Some(application_id) = application.strip_prefix(&prefix) else {
            panic!("case {case:?}: expected an ApplicationArn starting {prefix:?}, got {application:?}");
        };
        // `IdentityCenterApplicationArn` ends in `apl-[a-zA-Z0-9]{16}`.
        assert!(
            application_id.len() == 16 && application_id.chars().all(|c| c.is_ascii_alphanumeric()),
            "case {case:?}: expected the application id to match apl-[a-zA-Z0-9]{{16}}, \
             got {application_id:?}"
        );
        assert!(
            application_arns.insert(application.clone()),
            "case {case:?}: expected this web app's own application, got the repeat {application:?}"
        );
    }

    // Two web apps in the SAME instance get two applications.
    let first = create_web_app(
        &emulator,
        "the first web app of an instance",
        json!({"IdentityProviderDetails": default_identity_center()}),
    )
    .await;
    let second = create_web_app(
        &emulator,
        "the second web app of the same instance",
        json!({"IdentityProviderDetails": default_identity_center()}),
    )
    .await;
    let first = describe_web_app(&emulator, "the first web app of an instance", &first).await;
    let second =
        describe_web_app(&emulator, "the second web app of the same instance", &second).await;
    let first = member_of(described_identity_center(&first), "ApplicationArn").clone();
    let second = member_of(described_identity_center(&second), "ApplicationArn").clone();
    assert_ne!(
        first, second,
        "case \"two web apps in one instance\": expected a distinct ApplicationArn per web app, \
         got {first:?} twice"
    );
}

// ---------------------------------------------------------------------------
// The signing Region reaches the ARN and the derived endpoints.
// ---------------------------------------------------------------------------

/// RULE: a web app's ARN and its derived endpoints carry the Region the create was
/// signed for, not a fixed one.
///
/// The rows are Regions in three different partitions' commercial geographies, so a
/// handler that hardcoded one fails.
#[tokio::test]
async fn the_web_app_arn_and_endpoints_carry_the_signing_region() {
    let emulator = Emulator::new();

    for region in ["us-east-1", "us-west-2", "eu-central-1", "ap-southeast-2"] {
        let case = format!("a web app created in {region}");
        let web_app_id = create_web_app_in(
            &emulator,
            region,
            &case,
            json!({"IdentityProviderDetails": default_identity_center()}),
        )
        .await;
        let web_app = describe_web_app(&emulator, &case, &web_app_id).await;

        assert_members(
            &case,
            &web_app,
            &json!({
                "Arn": format!("arn:aws:transfer:{region}:{ACCOUNT}:webapp/{web_app_id}"),
                "WebAppEndpoint": format!("{web_app_id}.transfer-webapp.{region}.on.aws"),
                "AccessEndpoint": format!("https://{web_app_id}.transfer-webapp.{region}.on.aws"),
            }),
        );
    }
}

// ---------------------------------------------------------------------------
// A web app the store does not hold.
// ---------------------------------------------------------------------------

/// Syntactically valid `WebAppId`s (`len 24..24`, `^webapp-[0-9a-f]{17}$`) that no
/// `CreateWebApp` ever minted: the low end of the id space, a mid value, and the high
/// end.
const NEVER_MINTED: [&str; 3] = [
    "webapp-00000000000000000",
    "webapp-0000000000000dead",
    "webapp-fffffffffffffffff",
];

/// RULE: `DescribeWebApp` for a `WebAppId` the store does not hold answers this
/// service's not-found refusal — HTTP 400 `ResourceNotFoundException` with the
/// capitalized `ResourceType`, the requested identifier in `Resource` alone, and a
/// fixed generic `Message` that carries no identifier.
#[tokio::test]
async fn describing_a_web_app_that_was_never_created_answers_the_generic_404() {
    let emulator = Emulator::new();

    // A store that DOES hold a web app, so the refusal is about the id, not emptiness.
    create_web_app(
        &emulator,
        "a live web app",
        json!({"IdentityProviderDetails": default_identity_center()}),
    )
    .await;

    let mut messages: Vec<String> = Vec::new();

    for absent in NEVER_MINTED {
        assert!(
            is_modelled_web_app_id(absent),
            "the probe id {absent:?} must satisfy WebAppId's modelled pattern"
        );
        let answer = emulator
            .call("DescribeWebApp", json!({"WebAppId": absent}))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {absent:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {absent:?}: expected ResourceNotFoundException, got {}",
            answer.body
        );
        assert_members(
            absent,
            &answer.body,
            &json!({"Resource": absent, "ResourceType": "WebApp"}),
        );
        let message = member_of(&answer.body, "Message")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !message.is_empty() && !message.contains(absent),
            "case {absent:?}: expected a non-empty Message carrying no identifier, got {message:?}"
        );
        messages.push(message);
    }

    for message in &messages {
        assert_eq!(
            message, &messages[0],
            "expected one fixed WebApp not-found message, got {message:?} beside {:?}",
            messages[0]
        );
    }
}

// ---------------------------------------------------------------------------
// The store's own bound.
// ---------------------------------------------------------------------------

/// `MAX_STORED_WEB_APPS` in `src/state.rs`: the emulator's own bound on the web app
/// store.
const MAX_STORED_WEB_APPS: usize = 200;

/// RULE: the web app store is BOUNDED — at its capacity any further `CreateWebApp` is
/// refused with the modelled `ThrottlingException` (HTTP 429), mints nothing, and
/// leaves every stored web app readable.
///
/// The rows differ along what the refused create asks for, because the bound is a
/// property of the STORE, not of the request: a plain create, a VPC-hosted one, and a
/// tagged one with a custom access endpoint are all refused the same way.
#[tokio::test]
async fn the_web_app_store_is_bounded_and_refuses_every_create_past_its_bound() {
    let emulator = Emulator::new();

    let mut stored: Vec<String> = Vec::with_capacity(MAX_STORED_WEB_APPS);
    for index in 0..MAX_STORED_WEB_APPS {
        let case = format!("web app {index} of the bound");
        stored.push(
            create_web_app(
                &emulator,
                &case,
                json!({"IdentityProviderDetails": default_identity_center()}),
            )
            .await,
        );
    }

    let refused: Vec<(&str, Value)> = vec![
        (
            "a plain create at the bound",
            json!({"IdentityProviderDetails": default_identity_center()}),
        ),
        (
            "a VPC-hosted create at the bound",
            json!({
                "IdentityProviderDetails": default_identity_center(),
                "EndpointDetails": {"Vpc": {"VpcId": "vpc-0123456789abcdef0"}},
            }),
        ),
        (
            "a tagged create with a custom access endpoint at the bound",
            json!({
                "IdentityProviderDetails": default_identity_center(),
                "AccessEndpoint": "https://files.example.invalid",
                "Tags": [{"Key": "env", "Value": "dev"}],
            }),
        ),
    ];

    for (case, request) in refused {
        let answer = emulator.call("CreateWebApp", request).await;
        assert_eq!(
            answer.status, 429,
            "case {case:?}: expected HTTP 429, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ThrottlingException",
            "case {case:?}: expected ThrottlingException, got {}",
            answer.body
        );
        // `ThrottlingException` models only `RetryAfterSeconds`, and no documented
        // retry delay exists for this refusal, so none is reported.
        assert_members(
            case,
            &answer.body,
            &json!({"RetryAfterSeconds": null, "WebAppId": null}),
        );
    }

    // Nothing the bound refused displaced what the store already held.
    let first = stored.first().expect("the bound stored a first web app");
    let last = stored.last().expect("the bound stored a last web app");
    for web_app_id in [first, last] {
        let case = format!("the stored web app {web_app_id} after the refusals");
        let web_app = describe_web_app(&emulator, &case, web_app_id).await;
        assert_members(&case, &web_app, &json!({"WebAppId": web_app_id}));
    }
}
