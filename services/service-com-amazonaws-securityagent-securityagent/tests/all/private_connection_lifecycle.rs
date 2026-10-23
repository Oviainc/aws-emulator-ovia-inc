// SPDX-License-Identifier: CC0-1.0

//! The private connection surface of the SecurityAgent emulator, end to end over the
//! wire: `CreatePrivateConnection`, `DescribePrivateConnection`,
//! `ListPrivateConnections`.
//!
//! Every call goes through the real `MultiServiceRouter` the emulator boots, as a
//! `restJson1` `POST /<OperationName>` with a JSON body signed for the `securityagent`
//! signing name — the same harness `tests/all/application_lifecycle.rs` builds, reused
//! from there rather than written again. So nothing here can pass against a handler
//! that answers a canned body: a value must come back out of the shared store through
//! `DescribePrivateConnection` or `ListPrivateConnections` before a case counts.
//!
//! Each case asserts a RULE over a TABLE of inputs rather than one recorded call:
//! which members survive a round trip, what a reference the account does not hold
//! answers (and that it stores nothing), how an empty or ambiguous `mode` is worded,
//! where the clock-owed status transition lands, and what a `certificate` member has to
//! carry to be accepted.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_securityagent_securityagent::state::{
    default_vpc_id, region_subnets, CREATE_TRANSITION, DEFAULT_DNS_RESOLUTION,
    LATTICE_ID_SUFFIX_LEN, RESOURCE_CONFIGURATION_ID_PREFIX, RESOURCE_GATEWAY_ID_PREFIX,
    STATUS_ACTIVE, STATUS_CREATE_IN_PROGRESS, TYPE_SELF_MANAGED, TYPE_SERVICE_MANAGED,
};
use smithy_server_runtime::MultiServiceRouter;

// The wire harness — the router over one shared engine, and the signed `POST
// /<Operation>` call — lives in `crate::support`, shared by every suite in this test
// target rather than copied into one of them.
use crate::support::{call, emulator, member, Answer};

/// The region every case signs for, and so the region the account's network and the
/// ARNs below are read in.
const REGION: &str = "us-east-1";

/// The subnet identifiers the account's network has in [`REGION`], newest derivation
/// first — `region_subnets` is the account's own network, not request input.
fn account_subnets() -> Vec<String> {
    region_subnets(REGION)
        .into_iter()
        .map(|subnet| subnet.subnet_id)
        .collect()
}

/// `CreatePrivateConnection`, requiring the modelled 201 and naming the case when the
/// answer is anything else.
async fn create(router: &MultiServiceRouter, body: &Value, case: &str) -> Value {
    let answer = call(router, "CreatePrivateConnection", body).await;
    assert_eq!(
        answer.status, 201,
        "case {case:?}: expected CreatePrivateConnection to answer 201, got {} ({}) \
         with {}",
        answer.status, answer.error_code, answer.body
    );
    answer.body
}

/// `DescribePrivateConnection` for `name`, whatever it answers.
async fn describe(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        "DescribePrivateConnection",
        &json!({"privateConnectionName": name}),
    )
    .await
}

/// `DescribePrivateConnection` for `name`, requiring 200.
async fn described(router: &MultiServiceRouter, name: &str, case: &str) -> Value {
    let answer = describe(router, name).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected DescribePrivateConnection({name:?}) to answer 200, \
         got {} ({}) with {}",
        answer.status, answer.error_code, answer.body
    );
    answer.body
}

/// Every private connection name `ListPrivateConnections` reports, paging to the end
/// with `page_size` (or the service's default page size when `None`).
async fn list_names(router: &MultiServiceRouter, page_size: Option<i32>) -> Vec<String> {
    let mut names = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({});
        if let Some(size) = page_size {
            request["maxResults"] = json!(size);
        }
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let answer = call(router, "ListPrivateConnections", &request).await;
        assert_eq!(
            answer.status, 200,
            "case page size {page_size:?}: expected ListPrivateConnections to answer \
             200, got {} ({}) with {}",
            answer.status, answer.error_code, answer.body
        );
        let page = answer.body;
        let connections = page
            .get("privateConnections")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!("ListPrivateConnections answered no privateConnections list: {page}")
            })
            .clone();
        if let Some(size) = page_size {
            assert!(
                connections.len() <= size as usize,
                "case page size {size}: a page held {} connections: {page}",
                connections.len()
            );
        }
        for connection in &connections {
            names.push(
                member(connection, "name")
                    .unwrap_or_else(|| panic!("a summary carries no name: {connection}")),
            );
        }
        token = member(&page, "nextToken");
        if token.is_none() {
            break;
        }
        assert!(
            !connections.is_empty(),
            "case page size {page_size:?}: a page that carries a nextToken must not be \
             empty: {page}"
        );
        assert!(
            names.len() <= 1_000,
            "case page size {page_size:?}: pagination did not terminate"
        );
    }
    names
}

/// Whether `value` is a VPC Lattice id of `prefix`'s kind: the prefix plus exactly
/// `LATTICE_ID_SUFFIX_LEN` lowercase base-36 characters, the form
/// `com.amazonaws.vpclattice#ResourceGatewayId` (`^rgw-[0-9a-z]{17}$`) and
/// `#ResourceConfigurationId` (`^rcfg-[0-9a-z]{17}$`) declare.
fn is_lattice_id(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|suffix| {
        suffix.len() == LATTICE_ID_SUFFIX_LEN
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    })
}

/// One PEM-armoured certificate, as a `certificate` member carries it: the armour RFC
/// 7468 §5 states for an X.509 certificate around a base-64 body. The service reads the
/// envelope, not the X.509 inside it.
const CERTIFICATE_PEM: &str = concat!(
    "-----BEGIN CERTIFICATE-----\n",
    "MIIBdzCCAR2gAwIBAgIUAK1234567890abcdefghijklmnopqTAKBggqhkjOPQQD\n",
    "AjAWMRQwEgYDVQQDEwtwcm92aWRlcjEuZXhhbXBsZTAeFw0yNTAxMDEwMDAwMDBa\n",
    "-----END CERTIFICATE-----\n",
);

/// A chain of two such blocks: the leaf first, then its issuer, which is how a
/// `CertificateChain` carries more than one certificate.
fn certificate_chain_pem() -> String {
    format!("{CERTIFICATE_PEM}{CERTIFICATE_PEM}")
}

/// One service-managed `mode`, over the account's own network: `subnet_count` of the
/// Region's subnets, and whatever optional members the row adds.
fn service_managed(subnet_count: usize, extra: Value) -> Value {
    let mut mode = json!({
        "hostAddress": "provider.internal.example",
        "vpcId": default_vpc_id(REGION),
        "subnetIds": account_subnets().into_iter().take(subnet_count).collect::<Vec<_>>(),
    });
    if let Some(members) = extra.as_object() {
        for (name, value) in members {
            mode[name] = value.clone();
        }
    }
    json!({"serviceManaged": mode})
}

/// The rows the service-managed round trip spans. They differ along the dimensions the
/// rule generalises over: how many subnets the gateway spans, which optional members
/// the configuration carries, the DNS resolution mode, the IP address type, the number
/// of port ranges, and the size of the tag map.
///
/// The `ipAddressType` and `dnsResolution` values are the model's own catalogs
/// (`smithy_query(query="shape",shape="IpAddressType")`: IPV4 | IPV6 | DUAL_STACK;
/// `shape="ResourceConfigDnsResolution"`: PUBLIC | IN_VPC) — including the entries the
/// issue cited no request for.
fn service_managed_rows() -> Vec<(&'static str, Value, Value)> {
    vec![
        (
            "only the required members, one subnet",
            service_managed(1, json!({})),
            json!({}),
        ),
        (
            "two subnets and one tag",
            service_managed(2, json!({})),
            json!({"tags": {"env": "test"}}),
        ),
        (
            "every subnet of the Region, several tags",
            service_managed(3, json!({})),
            json!({"tags": {"env": "prod", "team": "appsec", "cost-center": "4711"}}),
        ),
        (
            "an explicit PUBLIC DNS resolution",
            service_managed(1, json!({"dnsResolution": "PUBLIC"})),
            json!({}),
        ),
        (
            "an IN_VPC DNS resolution",
            service_managed(2, json!({"dnsResolution": "IN_VPC"})),
            json!({}),
        ),
        (
            "an IPV4 gateway with one port range",
            service_managed(1, json!({"ipAddressType": "IPV4", "portRanges": ["443"]})),
            json!({}),
        ),
        (
            "an IPV6 gateway",
            service_managed(1, json!({"ipAddressType": "IPV6"})),
            json!({}),
        ),
        (
            "a DUAL_STACK gateway with several port ranges and addresses per ENI",
            service_managed(
                3,
                json!({
                    "ipAddressType": "DUAL_STACK",
                    "ipv4AddressesPerEni": 4,
                    "portRanges": ["80", "443", "8000-8100"],
                    "securityGroupIds": ["sg-0123456789abcdef0"],
                }),
            ),
            json!({"tags": {}}),
        ),
    ]
}

/// RULE: a create whose references the account resolves persists the connection and
/// answers the stored record — the caller's placement verbatim, the VPC Lattice
/// resources the service minted, and the status the clock owes it — and
/// `DescribePrivateConnection` reports that same record, not an echo of its own request.
#[tokio::test]
async fn create_then_describe_reads_back_the_stored_connection() {
    let router = emulator();
    let mut seen_gateways: BTreeSet<String> = BTreeSet::new();
    let mut seen_configurations: BTreeSet<String> = BTreeSet::new();

    for (index, (case, mode, extra)) in service_managed_rows().into_iter().enumerate() {
        let name = format!("privconn-{index}");
        let mut request = json!({"privateConnectionName": name, "mode": mode.clone()});
        if let Some(members) = extra.as_object() {
            for (member_name, value) in members {
                request[member_name] = value.clone();
            }
        }
        let created = create(&router, &request, case).await;

        let configuration = &mode["serviceManaged"];
        assert_eq!(
            member(&created, "name").as_deref(),
            Some(name.as_str()),
            "case {case:?}: expected the created connection to be named {name:?}, got \
             {created}"
        );
        assert_eq!(
            member(&created, "type").as_deref(),
            Some(TYPE_SERVICE_MANAGED),
            "case {case:?}: expected type {TYPE_SERVICE_MANAGED}, got {created}"
        );
        assert_eq!(
            member(&created, "status").as_deref(),
            Some(STATUS_CREATE_IN_PROGRESS),
            "case {case:?}: expected a create to answer the transient \
             {STATUS_CREATE_IN_PROGRESS}, got {created}"
        );

        // The gateway and the resource configuration are the service's to mint: real
        // VPC Lattice id forms, and a fresh pair per connection.
        let gateway = member(&created, "resourceGatewayId").unwrap_or_else(|| {
            panic!("case {case:?}: a service-managed connection reported no \
                    resourceGatewayId: {created}")
        });
        let configuration_id =
            member(&created, "resourceConfigurationId").unwrap_or_else(|| {
                panic!("case {case:?}: a service-managed connection reported no \
                        resourceConfigurationId: {created}")
            });
        assert!(
            is_lattice_id(&gateway, RESOURCE_GATEWAY_ID_PREFIX),
            "case {case:?}: expected a VPC Lattice resource gateway id, got {gateway:?}"
        );
        assert!(
            is_lattice_id(&configuration_id, RESOURCE_CONFIGURATION_ID_PREFIX),
            "case {case:?}: expected a VPC Lattice resource configuration id, got \
             {configuration_id:?}"
        );
        assert!(
            seen_gateways.insert(gateway.clone()),
            "case {case:?}: the resource gateway id {gateway:?} was minted twice"
        );
        assert!(
            seen_configurations.insert(configuration_id.clone()),
            "case {case:?}: the resource configuration id {configuration_id:?} was \
             minted twice"
        );

        // The placement is the caller's, reported back from the record.
        assert_eq!(
            member(&created, "hostAddress"),
            member(configuration, "hostAddress"),
            "case {case:?}: expected the stored hostAddress {:?}, got {:?}",
            member(configuration, "hostAddress"),
            member(&created, "hostAddress")
        );
        assert_eq!(
            member(&created, "vpcId"),
            member(configuration, "vpcId"),
            "case {case:?}: expected the stored vpcId {:?}, got {:?}",
            member(configuration, "vpcId"),
            member(&created, "vpcId")
        );
        // "Defaults to PUBLIC when not set" (`ServiceManagedInput$dnsResolution`).
        let expected_dns = member(configuration, "dnsResolution")
            .unwrap_or_else(|| DEFAULT_DNS_RESOLUTION.to_string());
        assert_eq!(
            member(&created, "dnsResolution").as_deref(),
            Some(expected_dns.as_str()),
            "case {case:?}: expected dnsResolution {expected_dns:?}, got {created}"
        );
        // No connection has failed, so nothing describes a failure.
        assert!(
            created.get("failureMessage").is_none(),
            "case {case:?}: expected no failureMessage on a connection that did not \
             fail, got {created}"
        );
        // A tag map is reported exactly when the connection carries tags.
        let expected_tags = extra
            .get("tags")
            .and_then(Value::as_object)
            .filter(|tags| !tags.is_empty())
            .cloned();
        assert_eq!(
            created.get("tags").and_then(Value::as_object).cloned(),
            expected_tags,
            "case {case:?}: expected tags {expected_tags:?}, got {created}"
        );

        // The round trip: every value above comes back out of the store.
        let read = described(&router, &name, case).await;
        for reported in ["name", "type", "resourceGatewayId", "hostAddress", "vpcId",
                         "resourceConfigurationId", "dnsResolution"] {
            assert_eq!(
                member(&read, reported),
                member(&created, reported),
                "case {case:?}: expected DescribePrivateConnection to report the \
                 stored {reported} {:?}, got {:?}",
                member(&created, reported),
                member(&read, reported)
            );
        }
        assert_eq!(
            read.get("tags").and_then(Value::as_object).cloned(),
            expected_tags,
            "case {case:?}: expected DescribePrivateConnection to report tags \
             {expected_tags:?}, got {read}"
        );
    }

    // Every created connection is in the listing, exactly once.
    let listed = list_names(&router, None).await;
    let expected: Vec<String> = (0..service_managed_rows().len())
        .map(|index| format!("privconn-{index}"))
        .collect();
    assert_eq!(
        listed, expected,
        "expected ListPrivateConnections to report every stored connection once, in \
         name order: expected {expected:?}, got {listed:?}"
    );
}

/// The references no account holds. They differ along the FORM of the reference (bare
/// id, this account's ARN, another region's, another account's, another vendor's, an
/// id of the wrong shape, free text) and along WHICH member carries it (the
/// self-managed resource configuration, the service-managed VPC, a service-managed
/// subnet).
fn unresolvable_rows() -> Vec<(&'static str, Value)> {
    let subnets = account_subnets();
    vec![
        (
            "a self-managed resource configuration id the account does not hold",
            json!({"selfManaged": {"resourceConfigurationId": "rcfg-0a1b2c3d4e5f60718"}}),
        ),
        (
            "its ARN in this account and region",
            json!({"selfManaged": {"resourceConfigurationId":
                "arn:aws:vpc-lattice:us-east-1:000000000000:resourceconfiguration/\
                 rcfg-0a1b2c3d4e5f60718"}}),
        ),
        (
            "an ARN naming another region",
            json!({"selfManaged": {"resourceConfigurationId":
                "arn:aws:vpc-lattice:eu-west-1:000000000000:resourceconfiguration/\
                 rcfg-0a1b2c3d4e5f60718"}}),
        ),
        (
            "an ARN naming another account",
            json!({"selfManaged": {"resourceConfigurationId":
                "arn:aws:vpc-lattice:us-east-1:111122223333:resourceconfiguration/\
                 rcfg-0a1b2c3d4e5f60718"}}),
        ),
        (
            "an ARN naming another vendor",
            json!({"selfManaged": {"resourceConfigurationId":
                "arn:aws:ec2:us-east-1:000000000000:resourceconfiguration/\
                 rcfg-0a1b2c3d4e5f60718"}}),
        ),
        (
            // An account exists in ONE partition, so a cross-partition ARN names
            // nothing this request can reach.
            "an ARN naming another partition",
            json!({"selfManaged": {"resourceConfigurationId":
                "arn:aws-us-gov:vpc-lattice:us-east-1:000000000000:\
                 resourceconfiguration/rcfg-0a1b2c3d4e5f60718"}}),
        ),
        (
            "an ARN naming another resource type",
            json!({"selfManaged": {"resourceConfigurationId":
                "arn:aws:vpc-lattice:us-east-1:000000000000:resourcegateway/\
                 rgw-0a1b2c3d4e5f60718"}}),
        ),
        (
            "an id of the wrong length",
            json!({"selfManaged": {"resourceConfigurationId": "rcfg-0a1b"}}),
        ),
        (
            "free text",
            json!({"selfManaged": {"resourceConfigurationId": "not-an-identifier"}}),
        ),
        (
            "a service-managed VPC the account does not have",
            json!({"serviceManaged": {
                "hostAddress": "provider.internal.example",
                "vpcId": "vpc-deadbeef",
                "subnetIds": [subnets[0].clone()],
            }}),
        ),
        (
            "a service-managed subnet the account does not have",
            json!({"serviceManaged": {
                "hostAddress": "provider.internal.example",
                "vpcId": default_vpc_id(REGION),
                "subnetIds": ["subnet-0000000000000000f"],
            }}),
        ),
        (
            "one subnet of the Region and one that does not exist",
            json!({"serviceManaged": {
                "hostAddress": "provider.internal.example",
                "vpcId": default_vpc_id(REGION),
                "subnetIds": [subnets[0].clone(), "subnet-0000000000000000f"],
            }}),
        ),
    ]
}

/// RULE: a create that references a resource the account does not hold is refused with
/// `ResourceNotFoundException` (HTTP 404) and persists NOTHING — the name it asked for
/// is afterwards unknown to `DescribePrivateConnection` and absent from
/// `ListPrivateConnections`.
#[tokio::test]
async fn a_reference_the_account_does_not_hold_is_not_found_and_stores_nothing() {
    let router = emulator();

    for (index, (case, mode)) in unresolvable_rows().into_iter().enumerate() {
        let name = format!("unresolvable-{index}");
        let answer = call(
            &router,
            "CreatePrivateConnection",
            &json!({"privateConnectionName": name, "mode": mode}),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException, got {} ({}) with {}",
            answer.status,
            answer.error_code,
            answer.body
        );

        let read = describe(&router, &name).await;
        assert_eq!(
            (read.status, read.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a refused create must persist nothing, but \
             DescribePrivateConnection({name:?}) answered {} ({}) with {}",
            read.status,
            read.error_code,
            read.body
        );
    }

    let listed = list_names(&router, None).await;
    assert!(
        listed.is_empty(),
        "expected a refused create to leave the listing empty, got {listed:?}"
    );
}

/// RULE: a self-managed create resolves the resource configuration the account DOES
/// hold, in either form the member admits ("The identifier or ARN of the resource
/// configuration"), and stores it as the account holds it — the bare id.
#[tokio::test]
async fn a_self_managed_create_resolves_a_resource_configuration_the_account_holds() {
    let router = emulator();

    // The account's resource configurations are the ones it created: one per
    // service-managed connection.
    let host = create(
        &router,
        &json!({
            "privateConnectionName": "host-connection",
            "mode": service_managed(1, json!({})),
        }),
        "the service-managed connection that creates the resource configuration",
    )
    .await;
    let configuration_id = member(&host, "resourceConfigurationId")
        .expect("the service-managed connection reports its resource configuration");

    // (case, the reference form the self-managed create names it by)
    let rows: Vec<(&str, String)> = vec![
        ("the bare id", configuration_id.clone()),
        (
            "its ARN in this account and region",
            format!(
                "arn:aws:vpc-lattice:{REGION}:000000000000:resourceconfiguration/\
                 {configuration_id}"
            ),
        ),
    ];

    for (index, (case, reference)) in rows.into_iter().enumerate() {
        let name = format!("self-managed-{index}");
        let created = create(
            &router,
            &json!({
                "privateConnectionName": name,
                "mode": {"selfManaged": {"resourceConfigurationId": reference}},
                "tags": {"mode": "self"},
            }),
            case,
        )
        .await;
        assert_eq!(
            member(&created, "type").as_deref(),
            Some(TYPE_SELF_MANAGED),
            "case {case:?}: expected type {TYPE_SELF_MANAGED}, got {created}"
        );
        assert_eq!(
            member(&created, "resourceConfigurationId").as_deref(),
            Some(configuration_id.as_str()),
            "case {case:?}: expected the resolved resource configuration \
             {configuration_id:?}, got {created}"
        );
        // The gateway behind a self-managed configuration is the caller's own and is
        // not named to the service, so there is none to report — and no VPC, no host
        // address and no DNS mode either.
        for absent in ["resourceGatewayId", "vpcId", "hostAddress", "dnsResolution"] {
            assert!(
                created.get(absent).is_none(),
                "case {case:?}: a self-managed connection must report no {absent}, got \
                 {created}"
            );
        }

        let read = described(&router, &name, case).await;
        assert_eq!(
            member(&read, "resourceConfigurationId").as_deref(),
            Some(configuration_id.as_str()),
            "case {case:?}: expected DescribePrivateConnection to report the stored \
             resource configuration {configuration_id:?}, got {read}"
        );
        assert_eq!(
            member(&read, "type").as_deref(),
            Some(TYPE_SELF_MANAGED),
            "case {case:?}: expected DescribePrivateConnection to report type \
             {TYPE_SELF_MANAGED}, got {read}"
        );
    }

    // An ARN addresses a resource of ONE partition, account and region, so the SAME
    // resource configuration named outside the one the request reaches resolves to
    // nothing — the only difference from the rows above is the field that moved.
    // (case, the reference form)
    let elsewhere: Vec<(&str, String)> = vec![
        (
            "its id in another partition",
            format!(
                "arn:aws-us-gov:vpc-lattice:{REGION}:000000000000:\
                 resourceconfiguration/{configuration_id}"
            ),
        ),
        (
            "its id in another region",
            format!(
                "arn:aws:vpc-lattice:eu-west-1:000000000000:resourceconfiguration/\
                 {configuration_id}"
            ),
        ),
        (
            "its id in another account",
            format!(
                "arn:aws:vpc-lattice:{REGION}:111122223333:resourceconfiguration/\
                 {configuration_id}"
            ),
        ),
    ];
    for (index, (case, reference)) in elsewhere.into_iter().enumerate() {
        let name = format!("elsewhere-{index}");
        let answer = call(
            &router,
            "CreatePrivateConnection",
            &json!({
                "privateConnectionName": name,
                "mode": {"selfManaged": {"resourceConfigurationId": reference}},
            }),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException, got {} ({}) with {}",
            answer.status,
            answer.error_code,
            answer.body
        );
    }
}

/// RULE: a `mode` that does not select exactly one of the union's members is refused
/// with `ValidationException` (HTTP 400) whose message names the REQUEST member and the
/// alternatives it permits — never the internal union shape.
#[tokio::test]
async fn a_mode_that_selects_no_single_member_names_the_member_and_its_alternatives() {
    let router = emulator();
    let subnets = account_subnets();

    // (case, the `mode` the request carries)
    let rows: Vec<(&str, Value)> = vec![
        ("no member at all", json!({})),
        (
            "both members at once",
            json!({
                "serviceManaged": {
                    "hostAddress": "provider.internal.example",
                    "vpcId": default_vpc_id(REGION),
                    "subnetIds": [subnets[0].clone()],
                },
                "selfManaged": {"resourceConfigurationId": "rcfg-0a1b2c3d4e5f60718"},
            }),
        ),
    ];

    for (index, (case, mode)) in rows.into_iter().enumerate() {
        let name = format!("bad-mode-{index}");
        let answer = call(
            &router,
            "CreatePrivateConnection",
            &json!({"privateConnectionName": name, "mode": mode}),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} ({}) with {}",
            answer.status,
            answer.error_code,
            answer.body
        );
        let message = member(&answer.body, "message")
            .or_else(|| member(&answer.body, "Message"))
            .unwrap_or_else(|| panic!("case {case:?}: the refusal carried no message: {}", answer.body));
        for named in ["mode", "serviceManaged", "selfManaged"] {
            assert!(
                message.contains(named),
                "case {case:?}: expected the refusal to name {named:?}, got {message:?}"
            );
        }
        assert!(
            !message.contains("PrivateConnectionMode") && !message.contains("union"),
            "case {case:?}: expected the refusal to name the request member rather than \
             the internal union shape, got {message:?}"
        );

        let read = describe(&router, &name).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a refused create must persist nothing, but \
             DescribePrivateConnection({name:?}) answered {} with {}",
            read.status, read.body
        );
    }
}

/// RULE: the name identifies the connection within the account, so a create under a
/// name the account already holds conflicts (HTTP 409 `ConflictException`) and leaves
/// the stored connection exactly as it was.
#[tokio::test]
async fn a_name_the_account_already_holds_conflicts() {
    let router = emulator();
    let first = create(
        &router,
        &json!({
            "privateConnectionName": "taken",
            "mode": service_managed(1, json!({})),
            "tags": {"generation": "first"},
        }),
        "the first connection under the name",
    )
    .await;

    // (case, the mode the second create asks for)
    let rows: Vec<(&str, Value)> = vec![
        ("the same service-managed placement", service_managed(1, json!({}))),
        ("a different service-managed placement", service_managed(3, json!({}))),
        (
            "a self-managed mode",
            json!({"selfManaged": {"resourceConfigurationId":
                member(&first, "resourceConfigurationId").unwrap_or_default()}}),
        ),
    ];

    for (case, mode) in rows {
        let answer = call(
            &router,
            "CreatePrivateConnection",
            &json!({"privateConnectionName": "taken", "mode": mode}),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (409, "ConflictException"),
            "case {case:?}: expected 409 ConflictException, got {} ({}) with {}",
            answer.status,
            answer.error_code,
            answer.body
        );
        let read = described(&router, "taken", case).await;
        assert_eq!(
            member(&read, "resourceGatewayId"),
            member(&first, "resourceGatewayId"),
            "case {case:?}: a refused create must not replace the stored connection"
        );
        assert_eq!(
            read.get("tags").and_then(Value::as_object).cloned(),
            first.get("tags").and_then(Value::as_object).cloned(),
            "case {case:?}: a refused create must not replace the stored tags"
        );
    }
}

/// RULE: a created connection is answered in the non-terminal
/// `CREATE_IN_PROGRESS` status and reaches `ACTIVE` on its own once the span the clock
/// owes it has passed — on every reader, and without a read moving it.
#[tokio::test]
async fn the_clock_owes_a_created_connection_its_active_status() {
    let router = emulator();

    // (case, the mode the connection is created with) — the transition is the same for
    // both modes, so both are spanned.
    let host = create(
        &router,
        &json!({
            "privateConnectionName": "host-connection",
            "mode": service_managed(1, json!({})),
        }),
        "the connection whose resource configuration the self-managed row references",
    )
    .await;
    let configuration_id =
        member(&host, "resourceConfigurationId").unwrap_or_default();
    let rows: Vec<(&str, Value)> = vec![
        ("a service-managed connection", service_managed(2, json!({}))),
        (
            "a self-managed connection",
            json!({"selfManaged": {"resourceConfigurationId": configuration_id}}),
        ),
    ];

    for (index, (case, mode)) in rows.into_iter().enumerate() {
        let name = format!("transition-{index}");
        let created = create(
            &router,
            &json!({"privateConnectionName": name, "mode": mode}),
            case,
        )
        .await;
        assert_eq!(
            member(&created, "status").as_deref(),
            Some(STATUS_CREATE_IN_PROGRESS),
            "case {case:?}: expected the create to answer {STATUS_CREATE_IN_PROGRESS}, \
             got {created}"
        );
        // Still transient straight after the call: the span is never zero, so a caller
        // acting immediately observes the transient status and not a resource that was
        // ready before it existed.
        let immediately = described(&router, &name, case).await;
        assert_eq!(
            member(&immediately, "status").as_deref(),
            Some(STATUS_CREATE_IN_PROGRESS),
            "case {case:?}: expected the connection to still be \
             {STATUS_CREATE_IN_PROGRESS} straight after its create, got {immediately}"
        );

        tokio::time::sleep(CREATE_TRANSITION + std::time::Duration::from_millis(250)).await;

        let later = described(&router, &name, case).await;
        assert_eq!(
            member(&later, "status").as_deref(),
            Some(STATUS_ACTIVE),
            "case {case:?}: expected the connection to reach {STATUS_ACTIVE} once the \
             span the clock owes it had passed, got {later}"
        );
        // The same status on the other reader, and reading it again does not move it
        // back.
        let again = described(&router, &name, case).await;
        assert_eq!(
            member(&again, "status").as_deref(),
            Some(STATUS_ACTIVE),
            "case {case:?}: expected a second read to report {STATUS_ACTIVE} too, got \
             {again}"
        );
    }
}

/// RULE: a `certificate` member is accepted only when it carries a PEM certificate
/// chain — a value that does not is refused with the modelled `ValidationException` and
/// stores nothing — and no connection reports a `certificateExpiryTime`, because this
/// emulator does not decode the X.509 inside the chain and so has no expiry to report.
#[tokio::test]
async fn a_certificate_is_accepted_only_as_a_pem_chain_and_yields_no_invented_expiry() {
    let router = emulator();

    // (case, the certificate the create supplies)
    let chain = certificate_chain_pem();
    let spaced = format!("\n{CERTIFICATE_PEM}\n");
    let rows: Vec<(&str, Option<&str>)> = vec![
        ("no certificate at all", None),
        ("one PEM certificate", Some(CERTIFICATE_PEM)),
        ("a chain of two certificates", Some(chain.as_str())),
        ("a chain surrounded by blank lines", Some(spaced.as_str())),
    ];

    for (index, (case, certificate)) in rows.into_iter().enumerate() {
        let name = format!("certificate-{index}");
        let mut mode = service_managed(1, json!({}));
        if let Some(certificate) = certificate {
            mode["serviceManaged"]["certificate"] = json!(certificate);
        }
        let created = create(
            &router,
            &json!({"privateConnectionName": name, "mode": mode}),
            case,
        )
        .await;

        // The member is absent rather than carrying an expiry nothing read.
        assert!(
            created.get("certificateExpiryTime").is_none(),
            "case {case:?}: expected no certificateExpiryTime, got {created}"
        );
        let read = described(&router, &name, case).await;
        assert!(
            read.get("certificateExpiryTime").is_none(),
            "case {case:?}: expected DescribePrivateConnection to report no \
             certificateExpiryTime, got {read}"
        );
        // The connection itself was stored: a well-formed chain does not stop a create.
        assert_eq!(
            member(&read, "name").as_deref(),
            Some(name.as_str()),
            "case {case:?}: expected the stored connection to be read back, got {read}"
        );
    }

    // (case, a value that is not a PEM-encoded certificate chain)
    let refused: Vec<(&str, &str)> = vec![
        ("free text", "not a certificate"),
        ("an empty value", ""),
        (
            "a PEM block of another kind",
            "-----BEGIN PRIVATE KEY-----\nZm9v\n-----END PRIVATE KEY-----\n",
        ),
        (
            "an opening line with no closing one",
            "-----BEGIN CERTIFICATE-----\nZm9v\n",
        ),
        (
            "armour around an empty body",
            "-----BEGIN CERTIFICATE-----\n\n-----END CERTIFICATE-----\n",
        ),
        (
            "armour around bytes outside the base-64 alphabet",
            "-----BEGIN CERTIFICATE-----\n!!not base64!!\n-----END CERTIFICATE-----\n",
        ),
    ];
    for (index, (case, certificate)) in refused.into_iter().enumerate() {
        let name = format!("bad-certificate-{index}");
        let mut mode = service_managed(1, json!({}));
        mode["serviceManaged"]["certificate"] = json!(certificate);
        let answer = call(
            &router,
            "CreatePrivateConnection",
            &json!({"privateConnectionName": name, "mode": mode}),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} ({}) with {}",
            answer.status,
            answer.error_code,
            answer.body
        );
        let read = describe(&router, &name).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a refused create must persist nothing, but \
             DescribePrivateConnection({name:?}) answered {} with {}",
            read.status, read.body
        );
    }
}

/// RULE: a listing reports every stored connection exactly once however it is paged,
/// and a cursor this listing did not mint is refused instead of being read as a resume
/// point.
#[tokio::test]
async fn a_listing_pages_through_every_stored_connection() {
    let router = emulator();
    let names: Vec<String> = (0..7).map(|index| format!("paged-{index}")).collect();
    for name in &names {
        create(
            &router,
            &json!({
                "privateConnectionName": name,
                "mode": service_managed(1, json!({})),
            }),
            name,
        )
        .await;
    }

    // (case, the page size the caller asks for)
    for (case, page_size) in [
        ("the service's default page size", None),
        ("one connection per page", Some(1)),
        ("two per page", Some(2)),
        ("three per page", Some(3)),
        ("more than the store holds", Some(1_000)),
    ] {
        let listed = list_names(&router, page_size).await;
        assert_eq!(
            listed, names,
            "case {case:?}: expected every stored connection once in name order: \
             expected {names:?}, got {listed:?}"
        );
    }

    // (case, a nextToken this listing did not mint)
    for (case, token) in [
        ("free text", "not-a-cursor"),
        ("a cursor of another listing", "app-v1:7061676564"),
        ("an empty cursor", ""),
    ] {
        let answer = call(
            &router,
            "ListPrivateConnections",
            &json!({"nextToken": token}),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException for a cursor this listing \
             did not mint, got {} ({}) with {}",
            answer.status,
            answer.error_code,
            answer.body
        );
    }
}
