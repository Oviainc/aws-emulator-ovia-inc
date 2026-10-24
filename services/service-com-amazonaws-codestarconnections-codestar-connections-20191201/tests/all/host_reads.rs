// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetHost`, the reporting half of the host resource.
//!
//! Every call goes over the wire: a request is dispatched through the
//! [`MultiServiceRouter`] exactly as the emulator's front door does it (`POST /` +
//! `X-Amz-Target: <prefix>.GetHost`, awsJson1_0 body), so what is asserted here is the
//! service's HTTP behaviour and not a handler called directly.
//!
//! RULE: `GetHost` REPORTS the host its `HostArn` names out of the service's state — each
//! modeled output member rebuilt from the stored record, the optional ones omitted when
//! the record carries none — and writes nothing, so neither the host's status nor
//! anything else a later caller sees moves because a host was read.
//!
//! `host_lifecycle.rs` pins the hosts `CreateHost` mints, every one of which is `PENDING`
//! with the pending status description. This suite SEEDS the store instead, which is the
//! only way to reach the rows no create produces: the four other statuses the
//! `Host$Status` doc names, a host carrying no status description, and a VPC configuration
//! with and without its optional `TlsCertificate`. Seeding writes the same store the
//! handler reads (`state_roundtrip.rs` pins that the engine is shared), so a seeded host
//! is observed over the wire exactly as a created one is.
//!
//! The harness is the crate's existing one ([`super::connection_lifecycle::Emulator`]);
//! the row table and the assertions here are `pub(crate)` because
//! `wire_target_routing.rs` drives the same rows behind the other target prefix forms.

use serde_json::{json, Value};

use service_com_amazonaws_codestarconnections_codestar_connections_20191201 as codestar;

use codestar::ops::types::VpcConfiguration;
use codestar::state::{
    ComAmazonawsCodestarconnectionsCodestarConnections20191201State as ServiceState, StoredHost,
    ARN_NAMESPACE, HOST_STATUS_AVAILABLE, HOST_STATUS_VALUES,
    HOST_STATUS_VPC_CONFIG_INITIALIZING,
};

// The one wire harness, the signing scope it sends, the provider-type catalog and the
// error-envelope readers live in `connection_lifecycle.rs`; this suite adds only what is
// specific to reading a host.
use super::connection_lifecycle::{
    error_code, error_message, Emulator, ACCOUNT_ID, PROVIDER_TYPES, REGION,
};

/// One row of the table: a host as state holds it, under the name of its case.
pub(crate) struct HostRow {
    pub(crate) case: &'static str,
    pub(crate) name: String,
    pub(crate) provider_type: &'static str,
    pub(crate) provider_endpoint: String,
    pub(crate) status: &'static str,
    pub(crate) status_message: Option<String>,
    pub(crate) vpc_configuration: Option<VpcConfiguration>,
}

/// A host ARN of the modeled `HostArn` form
/// `^arn:aws(-[\w]+)*:codestar-connections:.+:[0-9]{12}:host\/.+$`, in this request's
/// region and account.
pub(crate) fn host_arn(id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, REGION, &format!("host/{id}"))
}

/// Put `row` into the shared host store under an ARN built from `id`, as `CreateHost`
/// writes one, and return that ARN.
pub(crate) fn seed_host(emulator: &Emulator, id: &str, row: &HostRow) -> String {
    let host_arn = host_arn(id);
    let state = emulator.engine().state::<ServiceState>();
    let mut store = state.lock();
    // Creation order comes from the store's own counter, exactly as `CreateHost` takes it,
    // so two seeded hosts never share a sequence.
    let sequence = store.take_sequence();
    store.hosts.insert(
        host_arn.clone(),
        StoredHost {
            name: row.name.clone(),
            host_arn: host_arn.clone(),
            provider_type: row.provider_type.to_string(),
            provider_endpoint: row.provider_endpoint.clone(),
            vpc_configuration: row.vpc_configuration.clone(),
            status: row.status.to_string(),
            status_message: row.status_message.clone(),
            sequence,
        },
    );
    host_arn
}

/// Seed every row and return each row's ARN, in row order.
pub(crate) fn seed_rows(emulator: &Emulator, rows: &[HostRow]) -> Vec<String> {
    rows.iter()
        .enumerate()
        .map(|(index, row)| seed_host(emulator, &format!("seeded-{index}"), row))
        .collect()
}

/// Every stored host as `(ARN, status, status description)` — the state a read must leave
/// exactly as it found it.
pub(crate) fn stored_hosts(emulator: &Emulator) -> Vec<(String, String, Option<String>)> {
    emulator
        .engine()
        .state::<ServiceState>()
        .lock()
        .hosts
        .values()
        .map(|host| {
            (
                host.host_arn.clone(),
                host.status.clone(),
                host.status_message.clone(),
            )
        })
        .collect()
}

/// A `VpcConfiguration` with `subnets` subnet ids and `groups` security group ids, and a
/// TLS certificate only when `tls` says so.
///
/// `VpcId` keeps the modeled pattern `^vpc-\w{8}(\w{9})?$` (len 12..21); `SubnetIds` and
/// `SecurityGroupIds` are modeled `len 1..10`, which is what the counts span.
fn vpc_configuration(subnets: usize, groups: usize, tls: bool) -> VpcConfiguration {
    VpcConfiguration {
        vpc_id: if subnets > 1 {
            // The 21-character form: `vpc-` plus 17 word characters.
            "vpc-0a1b2c3d4e5f60718".to_string()
        } else {
            // The 12-character form: `vpc-` plus 8 word characters.
            "vpc-0a1b2c3d".to_string()
        },
        subnet_ids: (0..subnets).map(|i| format!("subnet-0a1b2c3d{i}")).collect(),
        security_group_ids: (0..groups).map(|i| format!("sg-0a1b2c3d{i}")).collect(),
        tls_certificate: tls.then(|| {
            "-----BEGIN CERTIFICATE-----\nZW11bGF0b3I=\n-----END CERTIFICATE-----".to_string()
        }),
    }
}

/// The table: one row per class the rule generalises over.
///
/// The statuses come from `state::HOST_STATUS_VALUES`, the generated catalog of the values
/// the `Host$Status` doc names ("such as PENDING, AVAILABLE, VPC_CONFIG_DELETING,
/// VPC_CONFIG_INITIALIZING, and VPC_CONFIG_FAILED_INITIALIZATION",
/// `smithy_query(query="shape", shape="Host")`); the provider types from the generated
/// `ProviderType` catalog. Rotating the two catalogs against each other covers every entry
/// of both, including the ones no issue cited, and the hand-written rows below sit on the
/// published `HostName` (`len 1..64`) and `Url` (`len 1..512`) bounds.
pub(crate) fn host_rows() -> Vec<HostRow> {
    let mut rows = vec![
        HostRow {
            case: "no vpc configuration and no status description",
            name: "ghes-plain".to_string(),
            provider_type: "GitHubEnterpriseServer",
            provider_endpoint: "https://ghes.example.internal".to_string(),
            status: HOST_STATUS_AVAILABLE,
            status_message: None,
            vpc_configuration: None,
        },
        HostRow {
            case: "one subnet, one security group, no tls certificate",
            name: "ghes-vpc-min".to_string(),
            provider_type: "GitHubEnterpriseServer",
            provider_endpoint: "https://ghes-vpc.example.internal:8443/path".to_string(),
            status: HOST_STATUS_VPC_CONFIG_INITIALIZING,
            status_message: Some("Initializing the VPC configuration".to_string()),
            vpc_configuration: Some(vpc_configuration(1, 1, false)),
        },
        HostRow {
            case: "ten subnets, ten security groups, tls certificate",
            name: "gitlab-vpc-max".to_string(),
            provider_type: "GitLabSelfManaged",
            provider_endpoint: "https://gitlab.example.internal".to_string(),
            status: "VPC_CONFIG_FAILED_INITIALIZATION",
            status_message: Some("The VPC configuration could not be initialized".to_string()),
            vpc_configuration: Some(vpc_configuration(10, 10, true)),
        },
        HostRow {
            // `HostName` is modeled `len 1..64`: the shortest name there is.
            case: "name at the lower length bound (1)",
            name: "h".to_string(),
            provider_type: "Bitbucket",
            provider_endpoint: "https://bitbucket.example.internal".to_string(),
            status: "VPC_CONFIG_DELETING",
            status_message: Some("Deleting the VPC configuration".to_string()),
            vpc_configuration: Some(vpc_configuration(2, 1, true)),
        },
        HostRow {
            // `HostName` is modeled `len 1..64`: the longest name there is.
            case: "name at the upper length bound (64)",
            name: "n".repeat(64),
            provider_type: "GitHub",
            provider_endpoint: "https://github.example.internal".to_string(),
            status: "PENDING",
            status_message: None,
            vpc_configuration: Some(vpc_configuration(3, 2, false)),
        },
        HostRow {
            // `Url` is modeled `len 1..512`: the shortest endpoint there is.
            case: "endpoint at the lower length bound (1)",
            name: "short-endpoint".to_string(),
            provider_type: "GitLab",
            provider_endpoint: "h".to_string(),
            status: HOST_STATUS_AVAILABLE,
            status_message: Some("Host is available".to_string()),
            vpc_configuration: None,
        },
        HostRow {
            // `Url` is modeled `len 1..512`: the longest endpoint there is.
            case: "endpoint at the upper length bound (512)",
            name: "long-endpoint".to_string(),
            provider_type: "GitHubEnterpriseServer",
            // Exactly 512 characters, computed from the surrounding literals so the bound
            // row cannot drift if they change.
            provider_endpoint: format!(
                "https://{}.example.net",
                "e".repeat(512 - "https://".len() - ".example.net".len())
            ),
            status: "PENDING",
            status_message: Some("Host is pending".to_string()),
            vpc_configuration: None,
        },
    ];
    // Every modeled provider type against every documented status, so no entry of either
    // catalog is left untested.
    for (index, provider_type) in PROVIDER_TYPES.iter().enumerate() {
        for (offset, status) in HOST_STATUS_VALUES.iter().enumerate() {
            rows.push(HostRow {
                case: "provider-type by status catalog row",
                name: format!("catalog-{provider_type}-{status}"),
                provider_type,
                provider_endpoint: format!(
                    "https://{}.example.internal",
                    provider_type.to_lowercase()
                ),
                status,
                status_message: ((index + offset) % 2 == 0)
                    .then(|| format!("status of a {provider_type} host: {status}")),
                vpc_configuration: ((index + offset) % 3 == 0)
                    .then(|| vpc_configuration(offset % 10 + 1, index % 10 + 1, offset % 2 == 0)),
            });
        }
    }
    assert!(
        rows.len() > PROVIDER_TYPES.len() * HOST_STATUS_VALUES.len(),
        "the table must span more than the two catalogs crossed, got {} rows",
        rows.len()
    );
    // The rows labelled as a published bound really sit on it (`HostName` len 1..64, `Url`
    // len 1..512), so a bound row cannot silently drift off the bound.
    for row in &rows {
        let (label, actual, expected) = match row.case {
            "name at the lower length bound (1)" => ("Name", row.name.len(), 1),
            "name at the upper length bound (64)" => ("Name", row.name.len(), 64),
            "endpoint at the lower length bound (1)" => {
                ("ProviderEndpoint", row.provider_endpoint.len(), 1)
            }
            "endpoint at the upper length bound (512)" => {
                ("ProviderEndpoint", row.provider_endpoint.len(), 512)
            }
            _ => continue,
        };
        assert_eq!(
            actual, expected,
            "case {:?}: {label} must be exactly {expected} characters, got {actual}",
            row.case
        );
    }
    rows
}

/// `VpcConfiguration` as the wire renders it: `TlsCertificate` is modeled optional, so a
/// configuration carrying none must not report the member at all.
fn vpc_configuration_json(vpc: &VpcConfiguration) -> Value {
    let mut rendered = json!({
        "VpcId": vpc.vpc_id,
        "SubnetIds": vpc.subnet_ids,
        "SecurityGroupIds": vpc.security_group_ids,
    });
    if let Some(certificate) = vpc.tls_certificate.as_deref() {
        rendered["TlsCertificate"] = json!(certificate);
    }
    rendered
}

/// Assert `response` is `row` as `GetHost` must report it: every modeled output member out
/// of the stored record, and nothing the output shape does not model.
pub(crate) fn assert_reports_row(case: &str, row: &HostRow, response: &Value) {
    assert_eq!(
        response["Name"],
        json!(row.name),
        "case {case:?}: wrong Name in {response:?}"
    );
    assert_eq!(
        response["Status"],
        json!(row.status),
        "case {case:?}: wrong Status in {response:?}"
    );
    assert_eq!(
        response["ProviderType"],
        json!(row.provider_type),
        "case {case:?}: wrong ProviderType in {response:?}"
    );
    assert_eq!(
        response["ProviderEndpoint"],
        json!(row.provider_endpoint),
        "case {case:?}: wrong ProviderEndpoint in {response:?}"
    );
    match row.vpc_configuration.as_ref() {
        Some(vpc) => assert_eq!(
            response["VpcConfiguration"],
            vpc_configuration_json(vpc),
            "case {case:?}: wrong VpcConfiguration in {response:?}"
        ),
        None => assert!(
            response.get("VpcConfiguration").is_none(),
            "case {case:?}: a host with no VPC configuration reported one: {response:?}"
        ),
    }
    // `GetHostOutput` models neither `HostArn` (the caller supplied it) nor
    // `StatusMessage`, so the answer must carry neither — whatever the record holds.
    for absent in ["HostArn", "StatusMessage"] {
        assert!(
            response.get(absent).is_none(),
            "case {case:?}: GetHostOutput models no {absent}, got {response:?}"
        );
    }
}

/// RULE: every modeled output member is reported out of the stored record, and the optional
/// ones the record does not carry are omitted — for every status the service reports, every
/// provider type, both published length bounds, and a VPC configuration with or without its
/// optional certificate.
#[tokio::test]
async fn get_host_reports_the_stored_record() {
    let emulator = Emulator::new();
    let rows = host_rows();
    let arns = seed_rows(&emulator, &rows);
    let before = stored_hosts(&emulator);

    for (row, arn) in rows.iter().zip(&arns) {
        let case = format!("{} ({})", row.case, row.name);
        let (status, response) = emulator.call("GetHost", json!({ "HostArn": arn })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 for a stored host, got {status} {response:?}"
        );
        assert_reports_row(&case, row, &response);
    }

    // A report writes nothing: every record is as it was seeded, status and description
    // included.
    assert_eq!(
        stored_hosts(&emulator),
        before,
        "reading every host changed what the store holds"
    );
}

/// RULE: `GetHost` reports — it never moves a host out of a status it was stored in, not
/// even one of the transitional VPC states, and repeating it answers the same thing.
///
/// A reporting operation may only materialise a change the clock already owes, and nothing
/// about a host's VPC provisioning is owed to a reader: a caller polling a host must see
/// the status the service holds, not one advanced by having been asked.
#[tokio::test]
async fn repeated_reads_report_the_same_host_and_move_nothing() {
    let emulator = Emulator::new();
    let rows = host_rows();
    let arns = seed_rows(&emulator, &rows);
    let before = stored_hosts(&emulator);

    for (row, arn) in rows.iter().zip(&arns) {
        let case = format!("{} ({})", row.case, row.name);
        let (first_status, first) = emulator.call("GetHost", json!({ "HostArn": arn })).await;
        let (second_status, second) = emulator.call("GetHost", json!({ "HostArn": arn })).await;
        assert_eq!(
            (first_status, &first),
            (second_status, &second),
            "case {case:?}: two reads of one host answered differently"
        );
        assert_eq!(
            first["Status"],
            json!(row.status),
            "case {case:?}: the read reported a status the record does not hold: {first:?}"
        );
    }

    assert_eq!(
        stored_hosts(&emulator),
        before,
        "repeated reads changed what the store holds"
    );
}

/// RULE: the lookup is by the host's own ARN — a host is reported only to the ARN it is
/// stored under, so one host's ARN never answers with another's record and an ARN that
/// resolves to nothing is the modelled absence.
#[tokio::test]
async fn each_host_is_reported_only_under_its_own_arn() {
    let emulator = Emulator::new();
    // Two rows that differ in every reported member, so a mixed-up lookup cannot pass.
    let rows = vec![
        HostRow {
            case: "the initializing host",
            name: "initializing".to_string(),
            provider_type: "GitHubEnterpriseServer",
            provider_endpoint: "https://initializing.example.internal".to_string(),
            status: HOST_STATUS_VPC_CONFIG_INITIALIZING,
            status_message: Some("Initializing the VPC configuration".to_string()),
            vpc_configuration: Some(vpc_configuration(2, 3, true)),
        },
        HostRow {
            case: "the available host",
            name: "available".to_string(),
            provider_type: "GitLab",
            provider_endpoint: "https://available.example.internal".to_string(),
            status: HOST_STATUS_AVAILABLE,
            status_message: None,
            vpc_configuration: None,
        },
    ];
    let arns = seed_rows(&emulator, &rows);

    for (row, arn) in rows.iter().zip(&arns) {
        let (status, response) = emulator.call("GetHost", json!({ "HostArn": arn })).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 for a stored host, got {status} {response:?}",
            row.case
        );
        assert_reports_row(row.case, row, &response);
    }

    // The same two hosts under ARNs nothing is stored under: the id is what resolves, so a
    // suffix, a different case or a different region is a different host.
    let stored = arns.first().expect("the table seeded two hosts");
    let unresolvable = [
        ("one extra character in the host id", format!("{stored}x")),
        (
            "the host id in another case",
            stored.replace("seeded-", "SEEDED-"),
        ),
        (
            "another region",
            format!("arn:aws:codestar-connections:eu-west-1:{ACCOUNT_ID}:host/seeded-0"),
        ),
    ];
    for (case, arn) in unresolvable {
        let (status, response) = emulator.call("GetHost", json!({ "HostArn": &arn })).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case:?}: expected the modelled absence for {arn:?}, got {status} {response:?}"
        );
        assert!(
            error_message(&response).contains(&arn),
            "case {case:?}: the refusal must name the ARN supplied, got {:?}",
            error_message(&response)
        );
    }
}
