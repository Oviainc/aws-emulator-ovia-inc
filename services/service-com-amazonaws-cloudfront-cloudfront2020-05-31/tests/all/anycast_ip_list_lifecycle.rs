// SPDX-License-Identifier: CC0-1.0

//! The CloudFront Anycast static IP list control plane, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization - the same path a
//! client takes. The rules under test are the ones CloudFront's Anycast static IP list
//! surface states:
//!
//! * a list is PERSISTED by the call that creates it, which MINTS its identity (an `Id`,
//!   the ARN built from it and a version token) and ALLOCATES the static IP addresses it
//!   holds, so a later `GetAnycastIpList` answers from that record rather than from the
//!   request being served - down to the address type a create left out;
//! * "The number of IP addresses in the Anycast static IP list" is the list's `IpCount`,
//!   so the list reports exactly that many addresses, each of a family its
//!   `IpAddressType` admits, and no address is ever allocated to two lists;
//! * "Valid values: 21 or 3": an `IpCount` outside the value set the member documents is
//!   refused and allocates nothing;
//! * a name identifies ONE list for its whole life, so a repeated name is refused and
//!   leaves the list that holds it untouched;
//! * a list CloudFront has just accepted is still `Deploying`, and the CLOCK - not a later
//!   call - owes it `Deployed`, and the IPAM CIDR ranges it was created with their
//!   provisioned status;
//! * an id no list carries is not found, whatever shape the id takes;
//! * an `AnycastIpListId` naming a list this service holds is an argument every operation
//!   that takes one resolves, while one naming no list is not.
//!
//! The table rows differ along everything these rules generalise over: the name's length
//! (both published bounds of `AnycastIpListName`) and which of its pattern's character
//! classes it draws from, both documented `IpCount` values, every `IpAddressType` the
//! model admits plus the request that names none, whether the create carries IPAM CIDR
//! configurations and how many, and whether it carries tags.
//!
//! The wire harness (the router, the request helper and the element scanner) is the one
//! [`crate::distribution_lifecycle`] already builds.

use std::collections::BTreeSet;
use std::time::Duration;

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    ANYCAST_IP_COUNTS, ANYCAST_IP_LIST_ID_HEX_LEN, ANYCAST_IP_LIST_ID_PREFIX,
    ANYCAST_IP_LIST_STATUS_DEPLOYING, ANYCAST_IP_POOL, IPAM_CIDR_STATUS_PROVISIONED,
    IPAM_CIDR_STATUS_PROVISIONING, IP_ADDRESS_TYPE_DUALSTACK, IP_ADDRESS_TYPE_IPV4,
    IP_ADDRESS_TYPE_IPV6, PROPAGATION_DELAY, STATUS_DEPLOYED,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::connection_groups::{create_connection_group, get_connection_group};
use crate::distribution_lifecycle::{
    call, emulator, is_version_token, Answer, Node, ACCOUNT_ID, API_VERSION,
};
use crate::key_value_store_lifecycle::member;
use crate::response_headers_policy_lifecycle::assert_absent;

// ---------------------------------------------------------------------------
// Calling the Anycast static IP list surface
// ---------------------------------------------------------------------------

/// `POST /2020-05-31/anycast-ip-list` with the request document `body`.
///
/// `pub(crate)` so the sibling listing suite drives the same create path rather than
/// growing its own copy of it.
pub(crate) async fn create_anycast_ip_list(router: &MultiServiceRouter, body: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/{API_VERSION}/anycast-ip-list"),
        Some(body),
        None,
    )
    .await
}

/// `GET /2020-05-31/anycast-ip-list/{Id}`.
pub(crate) async fn get_anycast_ip_list(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/anycast-ip-list/{id}"),
        None,
        None,
    )
    .await
}

// ---------------------------------------------------------------------------
// The creates under test
// ---------------------------------------------------------------------------

/// One `CreateAnycastIpList` request a client submits.
#[derive(Debug, Clone)]
pub(crate) struct Create {
    pub(crate) case: &'static str,
    pub(crate) name: String,
    pub(crate) ip_count: i32,
    /// `IpAddressType`, or `None` to leave the optional member out of the document
    /// entirely.
    pub(crate) ip_address_type: Option<&'static str>,
    /// `IpamCidrConfigs`, as `(Cidr, IpamPoolArn)` pairs.
    pub(crate) ipam_cidr_configs: Vec<(&'static str, &'static str)>,
    /// `Tags.Items`, as `(Key, Value)` pairs.
    pub(crate) tags: Vec<(&'static str, &'static str)>,
}

impl Create {
    /// The `CreateAnycastIpListRequest` XML a client sends for this create.
    ///
    /// `IpamCidrConfigList`'s member carries `xmlName` `IpamCidrConfig`
    /// (`smithy_query(query="shape",shape="IpamCidrConfigList")`), so each configuration
    /// travels in an element of that name.
    pub(crate) fn xml(&self) -> String {
        let mut out = format!(
            "<CreateAnycastIpListRequest \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <Name>{}</Name><IpCount>{}</IpCount>",
            self.name, self.ip_count
        );
        if !self.tags.is_empty() {
            out.push_str("<Tags><Items>");
            for (key, value) in &self.tags {
                out.push_str(&format!(
                    "<Tag><Key>{key}</Key><Value>{value}</Value></Tag>"
                ));
            }
            out.push_str("</Items></Tags>");
        }
        if let Some(ip_address_type) = self.ip_address_type {
            out.push_str(&format!(
                "<IpAddressType>{ip_address_type}</IpAddressType>"
            ));
        }
        if !self.ipam_cidr_configs.is_empty() {
            out.push_str("<IpamCidrConfigs>");
            for (cidr, pool_arn) in &self.ipam_cidr_configs {
                out.push_str(&format!(
                    "<IpamCidrConfig><Cidr>{cidr}</Cidr>\
                     <IpamPoolArn>{pool_arn}</IpamPoolArn></IpamCidrConfig>"
                ));
            }
            out.push_str("</IpamCidrConfigs>");
        }
        out.push_str("</CreateAnycastIpListRequest>");
        out
    }

    /// The `IpAddressType` the record must report: the one submitted, or the IPv4 family
    /// every Anycast static IP list carries when the create named none.
    pub(crate) fn expected_ip_address_type(&self) -> &str {
        self.ip_address_type.unwrap_or(IP_ADDRESS_TYPE_IPV4)
    }
}

/// The creates every rule below is asserted over.
///
/// `AnycastIpListName` is `length 1..64, pattern ^[a-zA-Z0-9-_]{1,64}$` and `IpCount` is
/// documented as "Valid values: 21 or 3" (both from `smithy_query` on
/// `CreateAnycastIpListRequest`), so the rows sit at both name bounds, draw from each
/// character class the pattern admits, use both documented counts, cover every
/// `IpAddressType` the model's enum admits plus a request that names none, and cover a
/// create with no IPAM CIDR configuration, one with several, and one with tags.
pub(crate) fn create_table() -> Vec<Create> {
    vec![
        Create {
            case: "the shortest admissible name, no optional member",
            name: "a".to_string(),
            ip_count: 3,
            ip_address_type: None,
            ipam_cidr_configs: Vec::new(),
            tags: Vec::new(),
        },
        Create {
            case: "the longest admissible name (64), the larger IpCount",
            name: "n".repeat(64),
            ip_count: 21,
            ip_address_type: None,
            ipam_cidr_configs: Vec::new(),
            tags: Vec::new(),
        },
        Create {
            case: "hyphens and underscores, ipv4 named explicitly",
            name: "edge-ips_2".to_string(),
            ip_count: 3,
            ip_address_type: Some(IP_ADDRESS_TYPE_IPV4),
            ipam_cidr_configs: Vec::new(),
            tags: Vec::new(),
        },
        Create {
            case: "mixed case and digits, ipv6 only",
            name: "EdgeIps42".to_string(),
            ip_count: 3,
            ip_address_type: Some(IP_ADDRESS_TYPE_IPV6),
            ipam_cidr_configs: Vec::new(),
            tags: Vec::new(),
        },
        Create {
            case: "dualstack, with tags",
            name: "dual_stack-list".to_string(),
            ip_count: 3,
            ip_address_type: Some(IP_ADDRESS_TYPE_DUALSTACK),
            ipam_cidr_configs: Vec::new(),
            tags: vec![("stage", "prod"), ("owner", "edge-team")],
        },
        Create {
            case: "two IPAM CIDR configurations, the larger IpCount",
            name: "ipam-sourced-list".to_string(),
            ip_count: 21,
            ip_address_type: Some(IP_ADDRESS_TYPE_IPV4),
            ipam_cidr_configs: vec![
                (
                    "198.51.100.0/25",
                    "arn:aws:ec2::000000000000:ipam-pool/ipam-pool-0a1b2c3d4e5f60718",
                ),
                (
                    "198.51.100.128/25",
                    "arn:aws:ec2::000000000000:ipam-pool/ipam-pool-0192837465abcdef0",
                ),
            ],
            tags: Vec::new(),
        },
    ]
}

// ---------------------------------------------------------------------------
// Reading what the service minted and allocated
// ---------------------------------------------------------------------------

/// The addresses one `AnycastIpList` document reports, in the order it reports them.
///
/// `AnycastIps`' member carries `xmlName` `AnycastIp`
/// (`smithy_query(query="shape",shape="AnycastIps")`), so each address travels in an
/// element of that name inside the `AnycastIps` wrapper.
pub(crate) fn anycast_ips(list: &Node) -> Vec<String> {
    match list.at("AnycastIps") {
        Some(wrapper) => wrapper
            .children_named("AnycastIp")
            .iter()
            .map(|address| address.text().to_string())
            .collect(),
        None => Vec::new(),
    }
}

/// Whether `value` has the shape CloudFront writes an Anycast static IP list id in: the
/// `ipl-` prefix followed by 17 lowercase hex digits, the shape this crate's
/// `ListConnectionGroups` cases already document for one.
fn is_anycast_ip_list_id(value: &str) -> bool {
    match value.strip_prefix(ANYCAST_IP_LIST_ID_PREFIX) {
        Some(body) => {
            body.len() == ANYCAST_IP_LIST_ID_HEX_LEN
                && body
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        }
        None => false,
    }
}

/// Whether `address` belongs to a family `ip_address_type` admits: an IPv4 address is
/// written in dotted quads and an IPv6 address in colon-separated groups, and `dualstack`
/// is "a list of both IPv4 and IPv6 addresses", so it admits either.
///
/// `pub(crate)` so the sibling suite that drives the UPDATE of a list's address type
/// decides the families the same way rather than growing a copy of this.
pub(crate) fn admits_family(ip_address_type: &str, address: &str) -> bool {
    match ip_address_type {
        IP_ADDRESS_TYPE_IPV6 => address.contains(':'),
        IP_ADDRESS_TYPE_DUALSTACK => address.contains(':') || address.contains('.'),
        _ => address.contains('.'),
    }
}

/// Assert the `AnycastIpList` document `answer` carries is the record `create` asked for,
/// in `status` with its IPAM ranges in `ipam_status`, and answer its `(Id, Arn,
/// AnycastIps)`.
fn assert_record(
    answer: &Answer,
    create: &Create,
    status: &str,
    ipam_status: &str,
    context: &str,
) -> (String, String, Vec<String>) {
    let case = create.case;
    let list = answer.root();
    let raw = answer.raw.as_str();

    let name = member(list, "Name", case, raw);
    assert_eq!(
        name, create.name,
        "case {case} ({context}): expected the submitted Name, got {name:?}"
    );

    // An Anycast static IP list id is minted in the shape CloudFront writes one in.
    let id = member(list, "Id", case, raw);
    assert!(
        is_anycast_ip_list_id(&id),
        "case {case} ({context}): expected a minted Id, got {id:?}"
    );

    let arn = member(list, "Arn", case, raw);
    assert_eq!(
        arn,
        format!("arn:aws:cloudfront::{ACCOUNT_ID}:anycast-ip-list/{id}"),
        "case {case} ({context}): expected the ARN built from the Id, got {arn:?}"
    );

    let actual_status = member(list, "Status", case, raw);
    assert_eq!(
        actual_status, status,
        "case {case} ({context}): expected Status {status}, got {actual_status:?}"
    );

    let ip_count = member(list, "IpCount", case, raw);
    assert_eq!(
        ip_count,
        create.ip_count.to_string(),
        "case {case} ({context}): expected the submitted IpCount, got {ip_count:?}"
    );

    let ip_address_type = member(list, "IpAddressType", case, raw);
    assert_eq!(
        ip_address_type,
        create.expected_ip_address_type(),
        "case {case} ({context}): expected IpAddressType {}, got {ip_address_type:?}",
        create.expected_ip_address_type()
    );

    // "The number of IP addresses in the Anycast static IP list": as many addresses as
    // the list's own IpCount, each of an admitted family.
    let addresses = anycast_ips(list);
    assert_eq!(
        i32::try_from(addresses.len()).unwrap_or(-1),
        create.ip_count,
        "case {case} ({context}): expected {} allocated addresses, got {addresses:?}",
        create.ip_count
    );
    for address in &addresses {
        assert!(
            admits_family(create.expected_ip_address_type(), address),
            "case {case} ({context}): {address:?} is no address of the {} family",
            create.expected_ip_address_type()
        );
    }
    // "dualstack - Allocate a list of both IPv4 and IPv6 addresses": both families are
    // present, which a single-family list cannot satisfy.
    if create.expected_ip_address_type() == IP_ADDRESS_TYPE_DUALSTACK {
        assert!(
            addresses.iter().any(|address| address.contains('.'))
                && addresses.iter().any(|address| address.contains(':')),
            "case {case} ({context}): expected both families, got {addresses:?}"
        );
    }
    assert_eq!(
        addresses.iter().collect::<BTreeSet<_>>().len(),
        addresses.len(),
        "case {case} ({context}): the same address was allocated twice: {addresses:?}"
    );

    let last_modified = member(list, "LastModifiedTime", case, raw);
    assert!(
        !last_modified.is_empty(),
        "case {case} ({context}): expected a LastModifiedTime, got {last_modified:?}"
    );

    // "The IPAM configuration for the Anycast static IP list, that contains the quantity
    // and list of IPAM CIDR configurations": reported only for a list that has one.
    if create.ipam_cidr_configs.is_empty() {
        assert_absent(list, "IpamConfig", case, context);
    } else {
        let ipam = list
            .at("IpamConfig")
            .unwrap_or_else(|| panic!("case {case} ({context}): the answer carries no IpamConfig: {raw}"));
        let quantity = member(ipam, "Quantity", case, raw);
        assert_eq!(
            quantity,
            create.ipam_cidr_configs.len().to_string(),
            "case {case} ({context}): expected Quantity {}, got {quantity:?}",
            create.ipam_cidr_configs.len()
        );
        let configs = match ipam.at("IpamCidrConfigs") {
            Some(wrapper) => wrapper.children_named("IpamCidrConfig"),
            None => Vec::new(),
        };
        assert_eq!(
            configs.len(),
            create.ipam_cidr_configs.len(),
            "case {case} ({context}): expected {} IPAM CIDR configurations, got {}",
            create.ipam_cidr_configs.len(),
            configs.len()
        );
        for (config, (cidr, pool_arn)) in configs.iter().zip(&create.ipam_cidr_configs) {
            assert_eq!(
                member(config, "Cidr", case, raw),
                *cidr,
                "case {case} ({context}): expected the submitted Cidr {cidr}"
            );
            assert_eq!(
                member(config, "IpamPoolArn", case, raw),
                *pool_arn,
                "case {case} ({context}): expected the submitted IpamPoolArn {pool_arn}"
            );
            let actual = member(config, "Status", case, raw);
            assert_eq!(
                actual, ipam_status,
                "case {case} ({context}): expected the IPAM CIDR status {ipam_status}, got \
                 {actual:?}"
            );
        }
    }

    (id, arn, addresses)
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// A create PERSISTS the list it mints and the addresses it allocates, and the read
/// answers from that one record. Across the table, no two lists share an id, an ARN, a
/// version token or a single allocated address.
#[tokio::test]
async fn create_persists_the_list_the_read_then_reports() {
    let router = emulator();
    let mut ids = BTreeSet::new();
    let mut arns = BTreeSet::new();
    let mut etags = BTreeSet::new();
    let mut allocated: BTreeSet<String> = BTreeSet::new();

    for create in create_table() {
        let case = create.case;
        let created = create_anycast_ip_list(&router, &create.xml()).await;
        assert_eq!(
            created.status, 202,
            "case {case}: expected HTTP 202, got {} with {}",
            created.status, created.raw
        );

        // A list CloudFront has just accepted has not finished deploying.
        let (id, arn, addresses) = assert_record(
            &created,
            &create,
            ANYCAST_IP_LIST_STATUS_DEPLOYING,
            IPAM_CIDR_STATUS_PROVISIONING,
            "the create",
        );

        let etag = created.header("etag").to_string();
        assert!(
            is_version_token(&etag),
            "case {case}: expected a version token in the ETag header, got {etag:?}"
        );
        assert!(ids.insert(id.clone()), "case {case}: the id {id} repeated");
        assert!(arns.insert(arn.clone()), "case {case}: the ARN {arn} repeated");
        assert!(
            etags.insert(etag.clone()),
            "case {case}: the version token {etag} repeated"
        );
        for address in &addresses {
            assert!(
                allocated.insert(address.clone()),
                "case {case}: the address {address} was allocated to another list too"
            );
        }

        // The read answers from the stored record, not from any request.
        let read = get_anycast_ip_list(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected HTTP 200 from the read, got {} with {}",
            read.status, read.raw
        );
        let (read_id, read_arn, read_addresses) = assert_record(
            &read,
            &create,
            ANYCAST_IP_LIST_STATUS_DEPLOYING,
            IPAM_CIDR_STATUS_PROVISIONING,
            "the read",
        );
        assert_eq!(
            (read_id, read_arn, read_addresses),
            (id, arn, addresses),
            "case {case}: the read must report the record the create stored"
        );
        let read_etag = read.header("etag").to_string();
        assert_eq!(
            read_etag, etag,
            "case {case}: expected the create's version token, got {read_etag:?}"
        );
    }
}

/// "Valid values: 21 or 3": an `IpCount` outside the documented value set is refused with
/// `InvalidArgument` (HTTP 400) and allocates nothing - the name it asked for is still
/// free afterwards. Each documented value is admitted.
#[tokio::test]
async fn an_ip_count_outside_the_documented_values_is_refused() {
    let router = emulator();

    // Rows: the empty allocation, one below and one above each documented value, and a
    // count no allocation could serve.
    let refused: [(&str, i32); 7] = [
        ("no addresses at all", 0),
        ("one below the smaller value", 2),
        ("one above the smaller value", 4),
        ("one below the larger value", 20),
        ("one above the larger value", 22),
        ("a negative count", -3),
        ("every address there could be", i32::MAX),
    ];

    for (case, ip_count) in refused {
        let name = format!("refused-{}", ip_count.unsigned_abs());
        let create = Create {
            case: "an IpCount outside the documented values",
            name: name.clone(),
            ip_count,
            ip_address_type: None,
            ipam_cidr_configs: Vec::new(),
            tags: Vec::new(),
        };
        let answer = create_anycast_ip_list(&router, &create.xml()).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400 for IpCount {ip_count}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgument",
            "case {case}: expected InvalidArgument for IpCount {ip_count}, got {}",
            answer.raw
        );

        // The refused create allocated nothing and took no name: the same name under a
        // documented count is accepted.
        let admitted = Create {
            ip_count: ANYCAST_IP_COUNTS[0],
            ..create
        };
        let answer = create_anycast_ip_list(&router, &admitted.xml()).await;
        assert_eq!(
            answer.status, 202,
            "case {case}: the refused create must leave the name {name} free, got {} with {}",
            answer.status, answer.raw
        );
    }
}

/// A name identifies ONE Anycast static IP list: the second create under a name is
/// refused with `EntityAlreadyExists` (HTTP 409), and the list that holds the name keeps
/// the identity, the addresses and the version token its own create minted.
#[tokio::test]
async fn a_name_names_at_most_one_anycast_ip_list() {
    let router = emulator();

    for create in create_table() {
        let case = create.case;
        let first = create_anycast_ip_list(&router, &create.xml()).await;
        assert_eq!(
            first.status, 202,
            "case {case}: expected HTTP 202, got {} with {}",
            first.status, first.raw
        );
        let (id, _, addresses) = assert_record(
            &first,
            &create,
            ANYCAST_IP_LIST_STATUS_DEPLOYING,
            IPAM_CIDR_STATUS_PROVISIONING,
            "the create",
        );
        let etag = first.header("etag").to_string();

        // The same name again - with the other documented count and no IPAM range, so a
        // list that was replaced rather than refused would be visible.
        let repeat = Create {
            ip_count: if create.ip_count == ANYCAST_IP_COUNTS[0] {
                ANYCAST_IP_COUNTS[1]
            } else {
                ANYCAST_IP_COUNTS[0]
            },
            ipam_cidr_configs: Vec::new(),
            ..create.clone()
        };
        let second = create_anycast_ip_list(&router, &repeat.xml()).await;
        assert_eq!(
            second.status, 409,
            "case {case}: expected HTTP 409 for a repeated name, got {} with {}",
            second.status, second.raw
        );
        assert_eq!(
            second.error_code(),
            "EntityAlreadyExists",
            "case {case}: expected EntityAlreadyExists, got {}",
            second.raw
        );

        let read = get_anycast_ip_list(&router, &id).await;
        let (read_id, _, read_addresses) = assert_record(
            &read,
            &create,
            ANYCAST_IP_LIST_STATUS_DEPLOYING,
            IPAM_CIDR_STATUS_PROVISIONING,
            "after a refusal",
        );
        assert_eq!(
            (read_id, read_addresses),
            (id, addresses),
            "case {case}: the refused create must not have replaced the list"
        );
        assert_eq!(
            read.header("etag"),
            etag,
            "case {case}: the refused create must not have spent a version token"
        );
    }
}

/// A list CloudFront has just created is `Deploying` with its IPAM ranges still being
/// provisioned, and the CLOCK owes it the terminal status of each: no call asks for the
/// transition, and the transition changes nothing else about the record - not its
/// identity, not its addresses, and not the version token an `If-Match` is checked
/// against.
#[tokio::test]
async fn the_clock_settles_deploying_to_deployed() {
    let router = emulator();
    let create = Create {
        case: "deployment settles",
        name: "settles_to_deployed".to_string(),
        ip_count: 3,
        ip_address_type: Some(IP_ADDRESS_TYPE_DUALSTACK),
        ipam_cidr_configs: vec![(
            "203.0.113.0/24",
            "arn:aws:ec2::000000000000:ipam-pool/ipam-pool-0abcdef1234567890",
        )],
        tags: Vec::new(),
    };

    let created = create_anycast_ip_list(&router, &create.xml()).await;
    assert_eq!(
        created.status, 202,
        "case {}: expected HTTP 202, got {} with {}",
        create.case, created.status, created.raw
    );
    let (id, arn, addresses) = assert_record(
        &created,
        &create,
        ANYCAST_IP_LIST_STATUS_DEPLOYING,
        IPAM_CIDR_STATUS_PROVISIONING,
        "the create",
    );
    let etag = created.header("etag").to_string();

    // A caller acting straight after the create still observes the transient status.
    let immediate = get_anycast_ip_list(&router, &id).await;
    assert_record(
        &immediate,
        &create,
        ANYCAST_IP_LIST_STATUS_DEPLOYING,
        IPAM_CIDR_STATUS_PROVISIONING,
        "straight after the create",
    );

    // Once the span has passed, the clock has settled the list - without any call having
    // asked for it.
    tokio::time::sleep(PROPAGATION_DELAY + Duration::from_millis(250)).await;

    let settled = get_anycast_ip_list(&router, &id).await;
    let (settled_id, settled_arn, settled_addresses) = assert_record(
        &settled,
        &create,
        STATUS_DEPLOYED,
        IPAM_CIDR_STATUS_PROVISIONED,
        "after the span",
    );
    assert_eq!(
        (settled_id, settled_arn, settled_addresses),
        (id, arn, addresses),
        "case {}: deploying must not change the list's identity or its addresses",
        create.case
    );
    assert_eq!(
        settled.header("etag"),
        etag,
        "case {}: a deployment is not a mutation, so the version token stands",
        create.case
    );
}

/// An id no list carries is not found - `EntityNotFound` (HTTP 404) - where the id is in
/// the shape an Anycast static IP list id takes, and an argument this operation cannot
/// read - `InvalidArgument` (HTTP 400) - where it is not; a create under one id leaves
/// every other id unfound.
#[tokio::test]
async fn a_read_refuses_an_id_no_list_holds() {
    let router = emulator();
    let present = Create {
        case: "the one list that exists",
        name: "present-list".to_string(),
        ip_count: 3,
        ip_address_type: None,
        ipam_cidr_configs: Vec::new(),
        tags: Vec::new(),
    };
    let created = create_anycast_ip_list(&router, &present.xml()).await;
    assert_eq!(
        created.status, 202,
        "case {}: expected HTTP 202, got {} with {}",
        present.case, created.status, created.raw
    );
    let id = member(created.root(), "Id", present.case, &created.raw);

    // Rows: an id in the shape this service mints but never minted, one of the same
    // namespace with a body of another form (the width and the alphabet of the body are one
    // observed id's, not the rule), the existing id in another case (the prefix names the
    // namespace, so an upper-cased id is in none), and the NAME of the list that does exist
    // - a list is addressed by its id, never by its name. The first two carry the namespace,
    // so they are answered as a list that is absent; the rest are refused as the unreadable
    // argument they are, before the store is consulted at all.
    let absent: [(&str, String, u16, &str); 4] = [
        (
            "an id never minted",
            format!("{ANYCAST_IP_LIST_ID_PREFIX}0123456789abcdef0"),
            404,
            "EntityNotFound",
        ),
        (
            "an id of the same namespace with another body",
            format!("{ANYCAST_IP_LIST_ID_PREFIX}notaminted-id"),
            404,
            "EntityNotFound",
        ),
        (
            "the existing id in upper case",
            id.to_uppercase(),
            400,
            "InvalidArgument",
        ),
        (
            "the name of the list that exists",
            present.name.clone(),
            400,
            "InvalidArgument",
        ),
    ];
    for (case, candidate, status, code) in absent {
        let answer = get_anycast_ip_list(&router, &candidate).await;
        assert_eq!(
            answer.status, status,
            "case {case}: expected HTTP {status} for {candidate:?}, got {} with {}",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            code,
            "case {case}: expected {code} for {candidate:?}, got {}",
            answer.raw
        );
    }
}

/// An `AnycastIpListId` names an Anycast static IP list, so the id of a list this service
/// HOLDS is an argument a connection group create resolves and keeps, while an id no list
/// carries is still the `InvalidArgument` every operation that takes one answers with.
#[tokio::test]
async fn an_anycast_ip_list_id_resolves_once_the_list_exists() {
    let router = emulator();
    let create = Create {
        case: "a list a connection group references",
        name: "referenced-list".to_string(),
        ip_count: 3,
        ip_address_type: None,
        ipam_cidr_configs: Vec::new(),
        tags: Vec::new(),
    };
    let created = create_anycast_ip_list(&router, &create.xml()).await;
    assert_eq!(
        created.status, 202,
        "case {}: expected HTTP 202, got {} with {}",
        create.case, created.status, created.raw
    );
    let id = member(created.root(), "Id", create.case, &created.raw);

    let group_body = |name: &str, anycast_ip_list_id: &str| {
        format!(
            "<CreateConnectionGroupRequest \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <Name>{name}</Name>\
             <AnycastIpListId>{anycast_ip_list_id}</AnycastIpListId>\
             </CreateConnectionGroupRequest>"
        )
    };

    let accepted = create_connection_group(&router, &group_body("resolved-group", &id)).await;
    assert_eq!(
        accepted.status, 201,
        "case {}: expected HTTP 201 for a group naming a list that exists, got {} with {}",
        create.case, accepted.status, accepted.raw
    );
    let read = get_connection_group(&router, "resolved-group").await;
    let held = member(
        read.root(),
        "AnycastIpListId",
        create.case,
        &read.raw,
    );
    assert_eq!(
        held, id,
        "case {}: expected the group to hold the list id it was created with, got {held:?}",
        create.case
    );

    // An id no list carries resolves to nothing, whatever shape it takes.
    for (case, candidate) in [
        (
            "an id never minted",
            format!("{ANYCAST_IP_LIST_ID_PREFIX}0123456789abcdef0"),
        ),
        ("the existing id in upper case", id.to_uppercase()),
    ] {
        let refused = create_connection_group(&router, &group_body("absent-group", &candidate)).await;
        assert_eq!(
            refused.status, 400,
            "case {case}: expected HTTP 400 for {candidate:?}, got {} with {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "InvalidArgument",
            "case {case}: expected InvalidArgument for {candidate:?}, got {}",
            refused.raw
        );
    }
}

/// Every address is allocated at most once, so the pool this emulator allocates from
/// bounds the lists it can hold: once it cannot serve a create's `IpCount`, the create is
/// refused with `EntityLimitExceeded` (HTTP 400) rather than handing out an address a
/// list already holds.
#[tokio::test]
async fn the_address_pool_bounds_what_can_be_allocated() {
    let router = emulator();
    let per_list = ANYCAST_IP_COUNTS[1];
    let lists = ANYCAST_IP_POOL / u32::try_from(per_list).expect("a documented count is positive");
    let mut allocated: BTreeSet<String> = BTreeSet::new();

    for index in 0..lists {
        let create = Create {
            case: "a list filling the pool",
            name: format!("pool-filler-{index}"),
            ip_count: per_list,
            ip_address_type: None,
            ipam_cidr_configs: Vec::new(),
            tags: Vec::new(),
        };
        let answer = create_anycast_ip_list(&router, &create.xml()).await;
        assert_eq!(
            answer.status, 202,
            "case list {index}: expected HTTP 202, got {} with {}",
            answer.status, answer.raw
        );
        for address in anycast_ips(answer.root()) {
            assert!(
                allocated.insert(address.clone()),
                "case list {index}: the address {address} was allocated twice"
            );
        }
    }

    // The pool now has fewer addresses left than the smallest documented count asks for.
    let beyond = Create {
        case: "one list past the pool",
        name: "past-the-pool".to_string(),
        ip_count: ANYCAST_IP_COUNTS[0],
        ip_address_type: None,
        ipam_cidr_configs: Vec::new(),
        tags: Vec::new(),
    };
    let answer = create_anycast_ip_list(&router, &beyond.xml()).await;
    assert_eq!(
        answer.status, 400,
        "case {}: expected HTTP 400 once the pool is spent, got {} with {}",
        beyond.case, answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        "EntityLimitExceeded",
        "case {}: expected EntityLimitExceeded, got {}",
        beyond.case,
        answer.raw
    );
    // The sentence CloudFront answers this code with on this operation: the Service Quotas
    // page of the quota that bounds the account's Anycast static IP lists, beside the
    // instruction to ask AWS Support for an increase. A LITERAL here rather than the crate's
    // constant, so a reworded refusal fails this suite.
    assert_eq!(
        answer.root().find_text("Message").unwrap_or_default(),
        "To request a limit increase for your Anycast static IP list, contact AWS Support: \
         https://us-east-1.console.aws.amazon.com/servicequotas/home/services/cloudfront/\
         quotas/L-6A19EDFD",
        "case {}: expected the service's sentence for this quota, got {}",
        beyond.case,
        answer.raw
    );
}
