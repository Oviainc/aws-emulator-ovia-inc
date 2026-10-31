// SPDX-License-Identifier: CC0-1.0

//! `UpdateAnycastIpList` — changing what an Anycast static IP list runs under, end to end
//! over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test:
//!
//! * the update submits an `IpAddressType` and `IpamCidrConfigs`, both optional, so a
//!   member the request names becomes the one the list runs under and a member it leaves
//!   out is left exactly as it was — and every later read answers from that record;
//! * the addresses "allocated to the Anycast static IP list" are the list's own for its
//!   whole life, so a list whose address type changes reports the SAME allocation in the
//!   families the new type admits, never a second one: no address is ever reported by two
//!   lists;
//! * the list's identity (`Id`, `Arn`, `Name`), its `IpCount` and its tag set are untouched
//!   — the update submits none of them;
//! * an update is a mutation: it mints a fresh version token, so the token the caller
//!   matched on no longer satisfies an `If-Match`, and the list re-enters its transient
//!   status with the CLOCK owing it the terminal one;
//! * an id no list carries is `EntityNotFound` (404) whatever its shape — this operation's
//!   own recorded answer, which differs from the read's — an `If-Match` that is no version
//!   token is `InvalidIfMatchVersion` (400) and one that is not the current version is
//!   `PreconditionFailed` (412) — and each refusal leaves the list exactly as it was.
//!
//! The rows differ along what those rules generalise over: the address type the list was
//! created with and the one it is updated to (every ordered pair the model's enum admits,
//! including the type the list already has), whether the update names the address type at
//! all, whether it names IPAM CIDR configurations and how many, and whether the list was
//! created with any.
//!
//! The harness (the router, the request helpers, the element scanner and the create
//! builder) is the [`crate::anycast_ip_list_lifecycle`] suite's, reused rather than copied.

use service_com_amazonaws_cloudfront_cloudfront2020_05_31::state::{
    ANYCAST_IP_LIST_STATUS_DEPLOYING, IPAM_CIDR_STATUS_PROVISIONED, IPAM_CIDR_STATUS_PROVISIONING,
    IP_ADDRESS_TYPE_DUALSTACK, IP_ADDRESS_TYPE_IPV4, IP_ADDRESS_TYPE_IPV6, STATUS_DEPLOYED,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::anycast_ip_list_lifecycle::{
    admits_family, anycast_ips, create_anycast_ip_list, get_anycast_ip_list, Create,
};
use crate::distribution_lifecycle::{
    call, emulator, is_version_token, past_propagation, Answer, API_VERSION,
};

/// The `IpAddressType` values the model admits, in the order
/// `smithy_query(query="shape",shape="IpAddressType")` lists them.
const IP_ADDRESS_TYPES: [&str; 3] = [
    IP_ADDRESS_TYPE_IPV4,
    IP_ADDRESS_TYPE_IPV6,
    IP_ADDRESS_TYPE_DUALSTACK,
];

/// `PUT /2020-05-31/anycast-ip-list/{Id}` with the members the row names, under `if_match`.
///
/// `IpamCidrConfigList`'s member carries `xmlName` `IpamCidrConfig`, so each configuration
/// travels in an element of that name — the same document the create submits them in.
async fn update_anycast_ip_list(
    router: &MultiServiceRouter,
    id: &str,
    if_match: Option<&str>,
    ip_address_type: Option<&str>,
    ipam_cidr_configs: Option<&[(&str, &str)]>,
) -> Answer {
    let mut body = format!(
        "<UpdateAnycastIpListRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">"
    );
    if let Some(ip_address_type) = ip_address_type {
        body.push_str(&format!(
            "<IpAddressType>{ip_address_type}</IpAddressType>"
        ));
    }
    if let Some(configs) = ipam_cidr_configs {
        body.push_str("<IpamCidrConfigs>");
        for (cidr, pool_arn) in configs {
            body.push_str(&format!(
                "<IpamCidrConfig><Cidr>{cidr}</Cidr>\
                 <IpamPoolArn>{pool_arn}</IpamPoolArn></IpamCidrConfig>"
            ));
        }
        body.push_str("</IpamCidrConfigs>");
    }
    body.push_str("</UpdateAnycastIpListRequest>");
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/anycast-ip-list/{id}"),
        Some(&body),
        if_match,
    )
    .await
}

/// The host SLOT each address names: the last group of an IPv6 address (written in hex)
/// and the last octet of an IPv4 one (written in decimal), so the same slot reads the same
/// whichever family it is written in.
fn host_slots(addresses: &[String]) -> Vec<u32> {
    addresses
        .iter()
        .map(|address| {
            if let Some(group) = address.rsplit(':').next().filter(|_| address.contains(':')) {
                u32::from_str_radix(group, 16).unwrap_or_default()
            } else {
                address
                    .rsplit('.')
                    .next()
                    .unwrap_or_default()
                    .parse()
                    .unwrap_or_default()
            }
        })
        .collect()
}

/// The IPAM CIDR ranges a document reports, as `(Cidr, IpamPoolArn, Status)`.
fn ipam_cidr_configs(answer: &Answer) -> Vec<(String, String, String)> {
    let Some(wrapper) = answer.root().descendant("IpamCidrConfigs") else {
        return Vec::new();
    };
    wrapper
        .children_named("IpamCidrConfig")
        .iter()
        .map(|config| {
            (
                config.text_at("Cidr").unwrap_or_default().to_string(),
                config.text_at("IpamPoolArn").unwrap_or_default().to_string(),
                config.text_at("Status").unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Create one Anycast static IP list of `ip_address_type` and report `(Id, ETag)`.
async fn created(
    router: &MultiServiceRouter,
    name: &str,
    ip_address_type: Option<&'static str>,
    ipam_cidr_configs: Vec<(&'static str, &'static str)>,
) -> (String, String) {
    let create = Create {
        case: "the list an update is asserted against",
        name: name.to_string(),
        ip_count: 3,
        ip_address_type,
        ipam_cidr_configs,
        tags: vec![("owner", "update-suite")],
    };
    let answer = create_anycast_ip_list(router, &create.xml()).await;
    assert_eq!(
        answer.status, 202,
        "case {name}: expected the create to answer 202, got {} ({})",
        answer.status, answer.raw
    );
    (
        answer.root().text_at("Id").unwrap_or_default().to_string(),
        answer.header("etag").to_string(),
    )
}

/// One row: the address type the list is created with, and the one the update names.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    created_as: Option<&'static str>,
    updated_to: Option<&'static str>,
    /// What the list's address type is after the update: the one named, or the one it
    /// already ran under where the update names none.
    expected: &'static str,
}

/// Every ordered pair of address types, plus the create that names none and the update
/// that names none — the two forms the optional member takes.
fn rows() -> Vec<Row> {
    let mut rows = vec![
        Row {
            case: "a create naming no type, an update naming none either",
            created_as: None,
            updated_to: None,
            expected: IP_ADDRESS_TYPE_IPV4,
        },
        Row {
            case: "a create naming no type, an update naming dualstack",
            created_as: None,
            updated_to: Some(IP_ADDRESS_TYPE_DUALSTACK),
            expected: IP_ADDRESS_TYPE_DUALSTACK,
        },
    ];
    for from in IP_ADDRESS_TYPES {
        for to in IP_ADDRESS_TYPES {
            rows.push(Row {
                case: "a create and an update naming a type each",
                created_as: Some(from),
                updated_to: Some(to),
                expected: to,
            });
        }
    }
    // A list that already runs under a type, updated without naming one: the type stands.
    rows.push(Row {
        case: "an update that names no type leaves the one the list runs under",
        created_as: Some(IP_ADDRESS_TYPE_IPV6),
        updated_to: None,
        expected: IP_ADDRESS_TYPE_IPV6,
    });
    rows
}

#[tokio::test]
async fn an_update_replaces_what_the_list_runs_under_and_keeps_its_allocation() {
    let router = emulator();

    for (index, row) in rows().into_iter().enumerate() {
        let case = format!("{} ({index}) {:?}->{:?}", row.case, row.created_as, row.updated_to);
        let name = format!("update-row-{index:02}");
        let (id, created_etag) = created(&router, &name, row.created_as, Vec::new()).await;

        // The allocation the create made, which the update must not take a second one for.
        let before = get_anycast_ip_list(&router, &id).await;
        let allocated = anycast_ips(before.root());
        assert_eq!(
            allocated.len(),
            3,
            "case {case}: expected the created list to hold 3 addresses, got {allocated:?}"
        );

        let updated =
            update_anycast_ip_list(&router, &id, Some(&created_etag), row.updated_to, None).await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected HTTP 200, got {} ({})",
            updated.status, updated.raw
        );

        // A fresh version token, so the one the caller matched on is spent.
        let new_etag = updated.header("etag").to_string();
        assert!(
            is_version_token(&new_etag),
            "case {case}: expected a minted version token, got {new_etag:?}"
        );
        assert_ne!(
            new_etag, created_etag,
            "case {case}: an update must mint a new version token, got {new_etag}"
        );

        // The type the list now runs under, and the allocation reported in its families.
        assert_eq!(
            updated.root().text_at("IpAddressType"),
            Some(row.expected),
            "case {case}: expected IpAddressType {:?}, got {}",
            row.expected,
            updated.raw
        );
        let reported = anycast_ips(updated.root());
        assert_eq!(
            reported.len(),
            allocated.len(),
            "case {case}: expected the same number of addresses, got {reported:?}"
        );
        for address in &reported {
            assert!(
                admits_family(row.expected, address),
                "case {case}: {address:?} is of no family {:?} admits",
                row.expected
            );
        }
        if row.expected == IP_ADDRESS_TYPE_DUALSTACK {
            assert!(
                reported.iter().any(|address| address.contains('.'))
                    && reported.iter().any(|address| address.contains(':')),
                "case {case}: dualstack holds both families, got {reported:?}"
            );
        }
        // The SLOTS are the list's own: the host numbers the create allocated are the ones
        // reported back, whichever family each is now written in — so an update takes
        // nothing more out of the service's address pool.
        assert_eq!(
            host_slots(&reported),
            host_slots(&allocated),
            "case {case}: expected the allocation {allocated:?} to stand, got {reported:?}"
        );

        // `IpCount`, the identity and the tag set are untouched.
        assert_eq!(
            updated.root().text_at("IpCount"),
            before.root().text_at("IpCount"),
            "case {case}: IpCount must stand, got {}",
            updated.raw
        );
        assert_eq!(
            updated.root().text_at("Id"),
            Some(id.as_str()),
            "case {case}: the id must stand, got {}",
            updated.raw
        );
        assert_eq!(
            updated.root().text_at("Name"),
            Some(name.as_str()),
            "case {case}: the name must stand, got {}",
            updated.raw
        );

        // An update has to reach the edge locations, so the list re-enters its transient
        // status and the CLOCK owes it the terminal one.
        assert_eq!(
            updated.root().text_at("Status"),
            Some(ANYCAST_IP_LIST_STATUS_DEPLOYING),
            "case {case}: expected {ANYCAST_IP_LIST_STATUS_DEPLOYING}, got {}",
            updated.raw
        );

        // The round trip: a read answers from the record the update wrote.
        let read = get_anycast_ip_list(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected the read to answer 200, got {} ({})",
            read.status, read.raw
        );
        assert_eq!(
            read.root().text_at("IpAddressType"),
            Some(row.expected),
            "case {case}: the read must answer from the updated record, got {}",
            read.raw
        );
        assert_eq!(
            anycast_ips(read.root()),
            reported,
            "case {case}: the read must report the same addresses, got {}",
            read.raw
        );
        assert_eq!(
            read.header("etag"),
            new_etag,
            "case {case}: the read must report the minted token, got {}",
            read.raw
        );

        // Past the span the clock owes, the list is deployed again — without another call
        // doing it.
        tokio::time::sleep(past_propagation()).await;
        let settled = get_anycast_ip_list(&router, &id).await;
        assert_eq!(
            settled.root().text_at("Status"),
            Some(STATUS_DEPLOYED),
            "case {case}: expected the clock to deploy the list, got {}",
            settled.raw
        );
    }
}

#[tokio::test]
async fn an_update_replaces_the_ipam_ranges_it_names_and_keeps_the_ones_it_does_not() {
    let router = emulator();

    // A list created with one range, and an update naming two others: the submitted set
    // REPLACES the stored one, and each range carries the status the service puts on a
    // range it has just been asked for.
    let (id, etag) = created(
        &router,
        "update-ipam-replaced",
        Some(IP_ADDRESS_TYPE_IPV4),
        vec![(
            "198.51.100.0/24",
            "arn:aws:ec2::000000000000:ipam-pool/ipam-pool-created",
        )],
    )
    .await;
    tokio::time::sleep(past_propagation()).await;
    let before = get_anycast_ip_list(&router, &id).await;
    assert_eq!(
        ipam_cidr_configs(&before)
            .iter()
            .map(|(_, _, status)| status.clone())
            .collect::<Vec<_>>(),
        vec![IPAM_CIDR_STATUS_PROVISIONED.to_string()],
        "case created: expected the clock to provision the created range, got {}",
        before.raw
    );

    let submitted: [(&str, &str); 2] = [
        (
            "203.0.113.0/24",
            "arn:aws:ec2::000000000000:ipam-pool/ipam-pool-first",
        ),
        (
            "203.0.113.0/25",
            "arn:aws:ec2::000000000000:ipam-pool/ipam-pool-second",
        ),
    ];
    let updated =
        update_anycast_ip_list(&router, &id, Some(&etag), None, Some(&submitted)).await;
    assert_eq!(
        updated.status, 200,
        "case replaced: expected HTTP 200, got {} ({})",
        updated.status, updated.raw
    );
    let expected: Vec<(String, String, String)> = submitted
        .iter()
        .map(|(cidr, arn)| {
            (
                (*cidr).to_string(),
                (*arn).to_string(),
                IPAM_CIDR_STATUS_PROVISIONING.to_string(),
            )
        })
        .collect();
    assert_eq!(
        ipam_cidr_configs(&updated),
        expected,
        "case replaced: expected {expected:?}, got {}",
        updated.raw
    );
    assert_eq!(
        updated.root().descendant("IpamConfig").and_then(|config| config.text_at("Quantity")),
        Some("2"),
        "case replaced: the quantity is read off the list, got {}",
        updated.raw
    );

    // The read answers the same ranges, and the clock provisions them.
    let read = get_anycast_ip_list(&router, &id).await;
    assert_eq!(
        ipam_cidr_configs(&read),
        expected,
        "case replaced: the read must answer from the updated record, got {}",
        read.raw
    );
    tokio::time::sleep(past_propagation()).await;
    let settled = get_anycast_ip_list(&router, &id).await;
    assert_eq!(
        ipam_cidr_configs(&settled)
            .iter()
            .map(|(_, _, status)| status.clone())
            .collect::<Vec<_>>(),
        vec![
            IPAM_CIDR_STATUS_PROVISIONED.to_string(),
            IPAM_CIDR_STATUS_PROVISIONED.to_string()
        ],
        "case replaced: expected the clock to provision the submitted ranges, got {}",
        settled.raw
    );

    // An update that names no ranges leaves the ones the list runs under.
    let kept = update_anycast_ip_list(
        &router,
        &id,
        Some(settled.header("etag")),
        Some(IP_ADDRESS_TYPE_IPV6),
        None,
    )
    .await;
    assert_eq!(
        kept.status, 200,
        "case kept: expected HTTP 200, got {} ({})",
        kept.status, kept.raw
    );
    assert_eq!(
        ipam_cidr_configs(&kept)
            .iter()
            .map(|(cidr, arn, _)| (cidr.clone(), arn.clone()))
            .collect::<Vec<_>>(),
        submitted
            .iter()
            .map(|(cidr, arn)| ((*cidr).to_string(), (*arn).to_string()))
            .collect::<Vec<_>>(),
        "case kept: the ranges the list runs under must stand, got {}",
        kept.raw
    );
}

#[tokio::test]
async fn an_update_is_refused_on_an_absent_list_and_on_a_version_that_is_not_current() {
    let router = emulator();
    let (id, etag) = created(&router, "update-refusals", Some(IP_ADDRESS_TYPE_IPV4), Vec::new())
        .await;

    // An id no list carries names nothing to update, WHATEVER shape it takes: this operation
    // resolves the id rather than reading its shape first, which is its own recorded answer
    // and differs from the read of the same resource. The rows span the forms an unusable id
    // takes all the same, so the one answer is asserted over the class.
    for (case, candidate) in [
        ("an id never minted", "ipl-00000000000000000".to_string()),
        (
            "an id of the same namespace with another body",
            "ipl-notaminted-id".to_string(),
        ),
        ("the stored id upper-cased", id.to_uppercase()),
        ("the list's name", "update-refusals".to_string()),
        ("a value of no id's namespace", "not-an-id".to_string()),
    ] {
        let answer = update_anycast_ip_list(
            &router,
            &candidate,
            Some(&etag),
            Some(IP_ADDRESS_TYPE_IPV6),
            None,
        )
        .await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404, got {} ({})",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            "EntityNotFound",
            "case {case}: expected EntityNotFound, got {:?} ({})",
            answer.error_code(),
            answer.raw
        );
    }

    // The precondition: a token that is no version token at all, and one that is well
    // formed but not the current version.
    for (case, if_match, status, code) in [
        (
            "a token of no version shape",
            Some("not-a-version"),
            400,
            "InvalidIfMatchVersion",
        ),
        (
            "a well-formed token that is not current",
            Some("E1PDK09ESLDGHD"),
            412,
            "PreconditionFailed",
        ),
    ] {
        let answer =
            update_anycast_ip_list(&router, &id, if_match, Some(IP_ADDRESS_TYPE_IPV6), None).await;
        assert_eq!(
            answer.status, status,
            "case {case}: expected HTTP {status}, got {} ({})",
            answer.status, answer.raw
        );
        assert_eq!(
            answer.error_code(),
            code,
            "case {case}: expected {code}, got {:?} ({})",
            answer.error_code(),
            answer.raw
        );
    }

    // Every refusal above left the list exactly as it was: the type it was created with,
    // under the token the create minted.
    let read = get_anycast_ip_list(&router, &id).await;
    assert_eq!(
        read.root().text_at("IpAddressType"),
        Some(IP_ADDRESS_TYPE_IPV4),
        "case refusals: the list must still run under the type it was created with, got {}",
        read.raw
    );
    assert_eq!(
        read.header("etag"),
        etag,
        "case refusals: a refused update must spend no version token, got {}",
        read.raw
    );
}
