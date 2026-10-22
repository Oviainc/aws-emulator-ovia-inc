// SPDX-License-Identifier: CC0-1.0

//! The Storage Gateway gateway / NFS-file-share lifecycle, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test is the one this control plane states for its two
//! primary resources: a gateway's and a file share's members are whatever the activating /
//! creating call stored, every reporting operation computes its answer from that record,
//! and an ARN that names no stored record is refused rather than answered.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_storagegateway_storagegateway_20130630::state::{
    FILE_SHARE_TRANSITION, MAX_FILE_SHARES, MAX_FILE_SHARES_PER_GATEWAY, MAX_GATEWAYS,
    MAX_PAGE_LIMIT,
};
use service_com_amazonaws_storagegateway_storagegateway_20130630::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service. It is what routes a request to the
/// storagegateway service, and its region is what the ARNs it mints name.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/storagegateway/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The region [`SIGNED_FOR_EU_WEST_1`] signs for.
const REGION: &str = "eu-west-1";

/// The canonical account id every emulated ARN carries, spelled out here rather than
/// imported so a wrong account in the implementation fails this test.
const ACCOUNT_ID: &str = "000000000000";

/// The `X-Amz-Target` prefix `OPERATIONS.md` gives for this service's awsJson1_1 routing.
const TARGET_PREFIX: &str = "StorageGateway_20130630";

/// An IAM role ARN matching `CreateNFSFileShareInput$Role`'s
/// `^arn:(aws(|-cn|-us-gov|-iso[A-Za-z0-9_-]*|-eusc)):iam::([0-9]+):role/(\S+)$`.
const ROLE: &str = "arn:aws:iam::000000000000:role/storage-gateway-share";

/// A gateway ARN of the right shape that no activation ever minted.
const NEVER_ACTIVATED_GATEWAY: &str =
    "arn:aws:storagegateway:eu-west-1:000000000000:gateway/sgw-DEADBEEF";

/// A file share ARN of the right shape that no create ever minted.
const NEVER_CREATED_SHARE: &str =
    "arn:aws:storagegateway:eu-west-1:000000000000:share/share-DEADBEEF";

/// The wire error code every bad-request refusal in this service carries: each of the 96
/// operations declares exactly `InternalServerError` (500) and this one (400).
const INVALID_GATEWAY_REQUEST: &str = "InvalidGatewayRequestException";

/// `GatewayId`'s 12..30, and the `sgw-` + hex form Storage Gateway ids take. Checked here
/// rather than through the crate's own code so a wrong id cannot make this test pass.
fn is_gateway_id(value: &str) -> bool {
    (12..=30).contains(&value.len())
        && value
            .strip_prefix("sgw-")
            .is_some_and(|hex| !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// `FileShareId`'s 12..30, and the `share-` + hex form.
fn is_file_share_id(value: &str) -> bool {
    (12..=30).contains(&value.len())
        && value
            .strip_prefix("share-")
            .is_some_and(|hex| !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// True when `value` is `arn:aws:storagegateway:<REGION>:<ACCOUNT_ID>:<kind>/<id>` and the
/// id satisfies `id_ok` — and when it is inside `GatewayARN` / `FileShareARN`'s 50..500.
fn is_resource_arn(value: &str, kind: &str, id_ok: fn(&str) -> bool) -> bool {
    if !(50..=500).contains(&value.len()) {
        return false;
    }
    let fields: Vec<&str> = value.split(':').collect();
    if fields.len() != 6 {
        return false;
    }
    let Some(id) = fields[5].strip_prefix(&format!("{kind}/")) else {
        return false;
    };
    fields[0] == "arn"
        && fields[1] == "aws"
        && fields[2] == "storagegateway"
        && fields[3] == REGION
        && fields[4] == ACCOUNT_ID
        && id_ok(id)
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), without any namespace prefix, or the empty string
    /// on a success.
    fn error_code(&self) -> &str {
        let raw = self.body["__type"].as_str().unwrap_or_default();
        match raw.rsplit_once('#') {
            Some((_, name)) => name,
            None => raw,
        }
    }

    /// A string member of the answer, or the empty string.
    fn string(&self, member: &str) -> String {
        self.body[member].as_str().unwrap_or_default().to_string()
    }
}

/// Call one operation, the way an SDK client does: `POST /` with `X-Amz-Target`.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> Answer {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "storagegateway.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_EU_WEST_1)
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header("content-type", "application/x-amz-json-1.1")
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// An activation request carrying only the four required members.
fn minimal_activation(name: &str) -> Value {
    json!({
        "ActivationKey": "activation-key-0001",
        "GatewayName": name,
        "GatewayTimezone": "GMT",
        "GatewayRegion": REGION,
    })
}

/// Activate a gateway that NFS file shares can live on, and answer its ARN.
async fn activate_file_gateway(router: &MultiServiceRouter, name: &str) -> String {
    let mut request = minimal_activation(name);
    request["GatewayType"] = json!("FILE_S3");
    let answer = call(router, "ActivateGateway", request).await;
    assert_eq!(
        answer.status, 200,
        "case {name}: expected HTTP 200 from ActivateGateway, got {} with {:?}",
        answer.status, answer.body
    );
    answer.string("GatewayARN")
}

/// A create request carrying only the four required members, plus the token.
fn minimal_share(gateway_arn: &str, token: &str) -> Value {
    json!({
        "ClientToken": token,
        "GatewayARN": gateway_arn,
        "Role": ROLE,
        "LocationARN": "arn:aws:s3:::amzn-s3-demo-bucket",
    })
}

/// Create one NFS file share and answer its ARN.
async fn create_share(router: &MultiServiceRouter, request: Value) -> String {
    let answer = call(router, "CreateNFSFileShare", request.clone()).await;
    assert_eq!(
        answer.status, 200,
        "expected HTTP 200 from CreateNFSFileShare for {request:?}, got {} with {:?}",
        answer.status, answer.body
    );
    answer.string("FileShareARN")
}

/// The one `NFSFileShareInfo` `DescribeNFSFileShares` reports for `arn`.
async fn describe_share(router: &MultiServiceRouter, arn: &str) -> Value {
    let answer = call(
        router,
        "DescribeNFSFileShares",
        json!({ "FileShareARNList": [arn] }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "expected HTTP 200 from DescribeNFSFileShares for {arn}, got {} with {:?}",
        answer.status, answer.body
    );
    let list = answer.body["NFSFileShareInfoList"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        list.len(),
        1,
        "DescribeNFSFileShares must report exactly the one share asked for, got {list:?}"
    );
    list[0].clone()
}

/// The `GatewayARN`s `ListGateways` reports, following `Marker` to the end.
async fn all_listed_gateways(router: &MultiServiceRouter) -> Vec<String> {
    let mut found = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let request = match &marker {
            Some(marker) => json!({ "Marker": marker }),
            None => json!({}),
        };
        let answer = call(router, "ListGateways", request).await;
        assert_eq!(
            answer.status, 200,
            "expected HTTP 200 from ListGateways, got {} with {:?}",
            answer.status, answer.body
        );
        for gateway in answer.body["Gateways"].as_array().cloned().unwrap_or_default() {
            found.push(
                gateway["GatewayARN"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        match answer.body["Marker"].as_str() {
            Some(next) => marker = Some(next.to_string()),
            None => return found,
        }
        assert!(
            found.len() < 1_000,
            "ListGateways pagination did not terminate, walked {found:?}"
        );
    }
}

/// The `FileShareInfo` entries `ListFileShares` reports for one request.
async fn list_shares(router: &MultiServiceRouter, request: Value) -> Answer {
    let answer = call(router, "ListFileShares", request.clone()).await;
    assert_eq!(
        answer.status, 200,
        "expected HTTP 200 from ListFileShares for {request:?}, got {} with {:?}",
        answer.status, answer.body
    );
    answer
}

/// Every `FileShareARN` `ListFileShares` reports, following `NextMarker` to the end.
async fn all_listed_shares(router: &MultiServiceRouter, gateway_arn: Option<&str>) -> Vec<String> {
    let mut found = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let mut request = json!({});
        if let Some(gateway_arn) = gateway_arn {
            request["GatewayARN"] = json!(gateway_arn);
        }
        if let Some(marker) = &marker {
            request["Marker"] = json!(marker);
        }
        let answer = list_shares(router, request).await;
        found.extend(listed_share_arns(&answer));
        match answer.body["NextMarker"].as_str() {
            Some(next) => marker = Some(next.to_string()),
            None => return found,
        }
        assert!(
            found.len() < 100_000,
            "ListFileShares pagination did not terminate, walked {} shares",
            found.len()
        );
    }
}

/// The `FileShareARN`s an answer's `FileShareInfoList` carries.
fn listed_share_arns(answer: &Answer) -> Vec<String> {
    answer.body["FileShareInfoList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|info| {
            info["FileShareARN"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The members of a create request that a describe must report back unchanged. Taken from
/// `CreateNFSFileShareInput` and `NFSFileShareInfo` in the Smithy model: every member the
/// two shapes share by name.
const ECHOED_SHARE_MEMBERS: &[&str] = &[
    "GatewayARN",
    "Role",
    "LocationARN",
    "FileShareName",
    "DefaultStorageClass",
    "ObjectACL",
    "ClientList",
    "Squash",
    "ReadOnly",
    "GuessMIMETypeEnabled",
    "RequesterPays",
    "CacheAttributes",
    "NotificationPolicy",
    "VPCEndpointDNSName",
    "BucketRegion",
    "AuditDestinationARN",
    "EncryptionType",
    "KMSEncrypted",
    "KMSKey",
    "Tags",
];

/// `ActivateGateway` stores the gateway the request describes, and every reporting
/// operation answers from that record: the ARN it mints is the key the gateway is held
/// under, `DescribeGatewayInformation` reports what was stored, and `ListGateways` carries
/// the same gateway.
///
/// The table spans the class the rule generalises over: each documented `GatewayType`
/// (including none at all, where the documented default applies), each documented
/// `GatewayTimezone` form, both ends of `GatewayName`'s 2..255, and 0, 1 and 3 tags.
#[tokio::test]
async fn activate_gateway_stores_the_gateway_every_report_then_computes() {
    struct Case {
        name: &'static str,
        gateway_name: String,
        gateway_type: Option<&'static str>,
        timezone: &'static str,
        tags: Vec<Value>,
        /// The type the gateway must report: the requested one, or the documented
        /// default when the request named none.
        expected_type: &'static str,
    }

    let cases = vec![
        Case {
            name: "no type at all takes the documented default",
            gateway_name: "gw".to_string(),
            gateway_type: None,
            timezone: "GMT",
            tags: vec![],
            expected_type: "CACHED",
        },
        Case {
            name: "an S3 file gateway, west of GMT",
            gateway_name: "my-gw".to_string(),
            gateway_type: Some("FILE_S3"),
            timezone: "GMT-5:00",
            tags: vec![json!({"Key": "team", "Value": "storage"})],
            expected_type: "FILE_S3",
        },
        Case {
            name: "a stored-volume gateway, east of GMT",
            gateway_name: "stored.gateway_1".to_string(),
            gateway_type: Some("STORED"),
            timezone: "GMT+10:00",
            tags: vec![
                json!({"Key": "team", "Value": "storage"}),
                json!({"Key": "env", "Value": "test"}),
                json!({"Key": "owner", "Value": "emulator"}),
            ],
            expected_type: "STORED",
        },
        Case {
            name: "a tape gateway with a drive and a medium changer",
            gateway_name: "tape gw".to_string(),
            gateway_type: Some("VTL"),
            timezone: "GMT-4:30",
            tags: vec![],
            expected_type: "VTL",
        },
        Case {
            name: "an FSx file gateway with the longest name the model admits",
            gateway_name: "g".repeat(255),
            gateway_type: Some("FILE_FSX_SMB"),
            timezone: "GMT+2:00",
            tags: vec![],
            expected_type: "FILE_FSX_SMB",
        },
    ];

    let router = emulator();
    let mut minted = Vec::new();

    for case in &cases {
        let mut request = minimal_activation(&case.gateway_name);
        request["GatewayTimezone"] = json!(case.timezone);
        request["Tags"] = json!(case.tags);
        if let Some(gateway_type) = case.gateway_type {
            request["GatewayType"] = json!(gateway_type);
        }
        if case.expected_type == "VTL" {
            request["TapeDriveType"] = json!("IBM-ULT3580-TD5");
            request["MediumChangerType"] = json!("STK-L700");
        }

        let activated = call(&router, "ActivateGateway", request).await;
        assert_eq!(
            activated.status, 200,
            "case {}: expected HTTP 200, got {} with {:?}",
            case.name, activated.status, activated.body
        );
        let arn = activated.string("GatewayARN");
        assert!(
            is_resource_arn(&arn, "gateway", is_gateway_id),
            "case {}: ActivateGateway must mint a well-formed gateway ARN, got {arn:?}",
            case.name
        );
        minted.push(arn.clone());

        let described = call(
            &router,
            "DescribeGatewayInformation",
            json!({ "GatewayARN": &arn }),
        )
        .await;
        assert_eq!(
            described.status, 200,
            "case {}: expected HTTP 200 from DescribeGatewayInformation, got {} with {:?}",
            case.name, described.status, described.body
        );
        assert_eq!(
            described.string("GatewayARN"),
            arn,
            "case {}: the described gateway must be the one the ARN names, got {:?}",
            case.name,
            described.body
        );
        assert_eq!(
            described.string("GatewayName"),
            case.gateway_name,
            "case {}: expected the stored name, got {:?}",
            case.name,
            described.body["GatewayName"]
        );
        assert_eq!(
            described.string("GatewayTimezone"),
            case.timezone,
            "case {}: expected the stored time zone, got {:?}",
            case.name,
            described.body["GatewayTimezone"]
        );
        assert_eq!(
            described.string("GatewayType"),
            case.expected_type,
            "case {}: expected gateway type {}, got {:?}",
            case.name,
            case.expected_type,
            described.body["GatewayType"]
        );
        assert_eq!(
            described.string("GatewayState"),
            "RUNNING",
            "case {}: an activated gateway is running, got {:?}",
            case.name,
            described.body["GatewayState"]
        );
        assert_eq!(
            described.body["Tags"],
            json!(case.tags),
            "case {}: expected the stored tags, got {:?}",
            case.name,
            described.body["Tags"]
        );
        let gateway_id = described.string("GatewayId");
        assert!(
            arn.ends_with(&format!("gateway/{gateway_id}")),
            "case {}: the reported GatewayId must be the one in the ARN, got {gateway_id:?} \
             for {arn:?}",
            case.name
        );
        assert_eq!(
            described.string("EndpointType"),
            "STANDARD",
            "case {}: expected the STANDARD endpoint type, got {:?}",
            case.name,
            described.body["EndpointType"]
        );
        assert!(
            !described.string("HostEnvironment").is_empty()
                && !described.string("SoftwareVersion").is_empty(),
            "case {}: a gateway reports its host environment and software version, got {:?}",
            case.name,
            described.body
        );
        assert!(
            described.body["GatewayNetworkInterfaces"]
                .as_array()
                .is_some_and(|interfaces| !interfaces.is_empty()),
            "case {}: a gateway reports at least one network interface, got {:?}",
            case.name,
            described.body["GatewayNetworkInterfaces"]
        );

        // The same gateway, read a second time, answers the same: a report changes
        // nothing a later caller sees.
        let again = call(
            &router,
            "DescribeGatewayInformation",
            json!({ "GatewayARN": &arn }),
        )
        .await;
        assert_eq!(
            again.body, described.body,
            "case {}: two reads of one gateway must answer the same, got {:?} then {:?}",
            case.name, described.body, again.body
        );
    }

    let distinct: BTreeSet<&String> = minted.iter().collect();
    assert_eq!(
        distinct.len(),
        minted.len(),
        "every activation must mint its own ARN, got {minted:?}"
    );

    let listed = all_listed_gateways(&router).await;
    assert_eq!(
        listed, minted,
        "ListGateways must report every activated gateway, in the order its ARNs sort"
    );

    // And each listed `GatewayInfo` is the same record, projected.
    let page = call(&router, "ListGateways", json!({})).await;
    for info in page.body["Gateways"].as_array().cloned().unwrap_or_default() {
        assert_eq!(
            info["GatewayOperationalState"],
            json!("ACTIVE"),
            "an activated gateway is ACTIVE in ListGateways, got {info:?}"
        );
        let arn = info["GatewayARN"].as_str().unwrap_or_default();
        let id = info["GatewayId"].as_str().unwrap_or_default();
        assert!(
            arn.ends_with(&format!("gateway/{id}")) && is_gateway_id(id),
            "a listed gateway's id must be the one in its ARN, got {info:?}"
        );
    }
}

/// `GatewayType` and `GatewayTimezone` are modelled by length alone, so the handler is
/// what holds their documented value set and form: `GatewayType` is one of
/// `STORED | CACHED | VTL | FILE_S3 | FILE_FSX_SMB`, and `GatewayTimezone` is `GMT`,
/// `GMT-hr:mm` or `GMT+hr:mm`. Anything else is a refused request, not a stored gateway.
#[tokio::test]
async fn activate_gateway_refuses_a_type_or_time_zone_the_service_does_not_accept() {
    let router = emulator();

    // (case, GatewayType, GatewayTimezone, accepted)
    let cases: &[(&str, &str, &str, bool)] = &[
        ("the documented type and the bare GMT form", "CACHED", "GMT", true),
        ("a one-digit hour west of GMT", "FILE_S3", "GMT-5:00", true),
        ("a two-digit hour east of GMT", "VTL", "GMT+10:00", true),
        ("a half-hour offset", "STORED", "GMT+5:30", true),
        ("the largest hour an offset can name", "CACHED", "GMT-23:59", true),
        ("a lower-cased gateway type", "cached", "GMT", false),
        ("a gateway type no value list names", "FILE_NFS", "GMT", false),
        ("a type that only looks like one", "CACHE", "GMT", false),
        ("a time zone that is not an offset at all", "CACHED", "PST", false),
        ("an offset missing its minutes", "CACHED", "GMT-5", false),
        ("an offset missing its sign", "CACHED", "GMT5:00", false),
        ("an offset with one minute digit", "CACHED", "GMT+5:0", false),
        ("an hour past the day", "CACHED", "GMT+24:00", false),
        ("a minute past the hour", "CACHED", "GMT+1:60", false),
    ];

    for (case, gateway_type, timezone, accepted) in cases {
        let mut request = minimal_activation("refusal-gw");
        request["GatewayType"] = json!(gateway_type);
        request["GatewayTimezone"] = json!(timezone);
        let answer = call(&router, "ActivateGateway", request).await;

        if *accepted {
            assert_eq!(
                answer.status, 200,
                "case {case}: GatewayType={gateway_type} GatewayTimezone={timezone} must be \
                 accepted, got {} with {:?}",
                answer.status, answer.body
            );
            continue;
        }

        assert_eq!(
            answer.status, 400,
            "case {case}: GatewayType={gateway_type} GatewayTimezone={timezone} must be \
             refused with HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            INVALID_GATEWAY_REQUEST,
            "case {case}: a refusal carries the one modelled client error, got {:?}",
            answer.body
        );
    }

    // Nothing refused was stored: only the accepted rows are gateways.
    let accepted = cases.iter().filter(|(_, _, _, ok)| *ok).count();
    let listed = all_listed_gateways(&router).await;
    assert_eq!(
        listed.len(),
        accepted,
        "a refused activation must store nothing: expected {accepted} gateways, got {listed:?}"
    );
}

/// The modelled bounds are refused before any handler runs, so a value outside one is a
/// 400 and no stored record — on both ends of the range, and one past each.
#[tokio::test]
async fn activate_gateway_refuses_a_value_outside_a_modelled_bound() {
    let router = emulator();

    // (case, member, value, inside the bound)
    let cases: &[(&str, &str, String, bool)] = &[
        ("ActivationKey at its 1-character minimum", "ActivationKey", "k".to_string(), true),
        ("ActivationKey at its 50-character maximum", "ActivationKey", "k".repeat(50), true),
        ("ActivationKey one past its maximum", "ActivationKey", "k".repeat(51), false),
        ("ActivationKey one short of its minimum", "ActivationKey", String::new(), false),
        ("GatewayName at its 2-character minimum", "GatewayName", "gw".to_string(), true),
        ("GatewayName at its 255-character maximum", "GatewayName", "g".repeat(255), true),
        ("GatewayName one past its maximum", "GatewayName", "g".repeat(256), false),
        ("GatewayName one short of its minimum", "GatewayName", "g".to_string(), false),
    ];

    let mut accepted = 0;
    for (case, member, value, inside) in cases {
        let mut request = minimal_activation("bounds-gw");
        request[*member] = json!(value);
        let answer = call(&router, "ActivateGateway", request).await;
        if *inside {
            accepted += 1;
            assert_eq!(
                answer.status, 200,
                "case {case}: {member} must be accepted, got {} with {:?}",
                answer.status, answer.body
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case}: {member} must be refused with HTTP 400, got {} with {:?}",
                answer.status, answer.body
            );
        }
    }

    let listed = all_listed_gateways(&router).await;
    assert_eq!(
        listed.len(),
        accepted,
        "only the accepted rows may be stored: expected {accepted} gateways, got {listed:?}"
    );
}

/// `CreateNFSFileShare` stores every member the request carried, and
/// `DescribeNFSFileShares` reconstructs the share from that record — so nothing a caller
/// supplied is silently dropped, and every member it left out takes the default its own
/// documentation states.
///
/// The table spans the class: the bare required members, every optional member at once,
/// and rows differing in client-list size, squash mapping, storage class, ACL, and how
/// much of `NFSFileShareDefaults` is given.
#[tokio::test]
async fn nfs_file_share_reports_back_every_member_the_create_stored() {
    let router = emulator();
    let gateway = activate_file_gateway(&router, "share-gw").await;

    let mut full = minimal_share(&gateway, "token-full-00001");
    full["LocationARN"] = json!("arn:aws:s3:::amzn-s3-demo-bucket/prefix/");
    full["FileShareName"] = json!("full-share");
    full["DefaultStorageClass"] = json!("S3_STANDARD_IA");
    full["ObjectACL"] = json!("bucket-owner-full-control");
    full["ClientList"] = json!(["10.0.0.0/8", "192.0.2.4"]);
    full["Squash"] = json!("AllSquash");
    full["ReadOnly"] = json!(true);
    full["GuessMIMETypeEnabled"] = json!(false);
    full["RequesterPays"] = json!(true);
    full["NFSFileShareDefaults"] = json!({
        "FileMode": "0640",
        "DirectoryMode": "0750",
        "GroupId": 1001,
        "OwnerId": 1002,
    });
    full["CacheAttributes"] = json!({ "CacheStaleTimeoutInSeconds": 300 });
    full["NotificationPolicy"] = json!("{\"Upload\": {\"SettlingTimeInSeconds\": 60}}");
    full["BucketRegion"] = json!(REGION);
    full["VPCEndpointDNSName"] = json!("bucket.vpce-1a2b3c4d-5e6f.s3.eu-west-1.vpce.amazonaws.com");
    full["AuditDestinationARN"] =
        json!("arn:aws:logs:eu-west-1:000000000000:log-group:gateway-audit");
    full["EncryptionType"] = json!("SseKms");
    full["KMSEncrypted"] = json!(true);
    full["KMSKey"] =
        json!("arn:aws:kms:eu-west-1:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab");
    full["Tags"] = json!([
        {"Key": "team", "Value": "storage"},
        {"Key": "env", "Value": "test"},
    ]);

    let mut many_clients = minimal_share(&gateway, "token-clients-001");
    many_clients["ClientList"] = json!((0..24)
        .map(|index| format!("198.51.100.{index}"))
        .collect::<Vec<_>>());
    many_clients["Squash"] = json!("NoSquash");
    many_clients["ObjectACL"] = json!("private");

    let mut partial_defaults = minimal_share(&gateway, "token-partial-01");
    partial_defaults["NFSFileShareDefaults"] = json!({ "FileMode": "0600" });
    partial_defaults["DefaultStorageClass"] = json!("S3_ONEZONE_IA");
    partial_defaults["ReadOnly"] = json!(false);

    let mut one_client = minimal_share(&gateway, "token-one-client1");
    one_client["ClientList"] = json!(["203.0.113.7"]);
    one_client["Squash"] = json!("RootSquash");
    one_client["GuessMIMETypeEnabled"] = json!(true);
    one_client["DefaultStorageClass"] = json!("S3_INTELLIGENT_TIERING");

    let cases = vec![
        ("the bare required members", minimal_share(&gateway, "token-minimal-01")),
        ("every optional member at once", full),
        ("a long client list and no squash", many_clients),
        ("NFS defaults given only in part", partial_defaults),
        ("a single client and MIME guessing on", one_client),
    ];

    let mut arns = Vec::new();
    for (case, request) in &cases {
        let arn = create_share(&router, request.clone()).await;
        assert!(
            is_resource_arn(&arn, "share", is_file_share_id),
            "case {case}: CreateNFSFileShare must mint a well-formed share ARN, got {arn:?}"
        );
        arns.push(arn.clone());

        let described = describe_share(&router, &arn).await;
        let supplied: &Map<String, Value> = request.as_object().expect("the request is an object");

        for member in ECHOED_SHARE_MEMBERS {
            let Some(expected) = supplied.get(*member) else {
                continue;
            };
            assert_eq!(
                &described[*member], expected,
                "case {case}: {member} must come back as supplied, expected {expected:?}, \
                 got {:?}",
                described[*member]
            );
        }

        // `NFSFileShareDefaults` is reported whole, so the members the request named
        // keep their value and the rest take their documented default.
        if let Some(defaults) = supplied.get("NFSFileShareDefaults").and_then(Value::as_object) {
            for (member, expected) in defaults {
                assert_eq!(
                    &described["NFSFileShareDefaults"][member], expected,
                    "case {case}: NFSFileShareDefaults.{member} must come back as supplied, \
                     expected {expected:?}, got {:?}",
                    described["NFSFileShareDefaults"][member]
                );
            }
        }

        assert_eq!(
            described["FileShareARN"],
            json!(&arn),
            "case {case}: the described share must be the one the ARN names, got {described:?}"
        );
        assert_eq!(
            described["GatewayARN"],
            json!(&gateway),
            "case {case}: a share belongs to the gateway it was created on, got {described:?}"
        );
        assert_eq!(
            described["Path"],
            json!("/"),
            "case {case}: an S3 file share is mounted at its root, got {:?}",
            described["Path"]
        );
        let id = described["FileShareId"].as_str().unwrap_or_default();
        assert!(
            arn.ends_with(&format!("share/{id}")) && is_file_share_id(id),
            "case {case}: the reported FileShareId must be the one in the ARN, got {id:?} \
             for {arn:?}"
        );

        // Documented defaults, for every member this row left out.
        let defaulted: &[(&str, Value)] = &[
            ("DefaultStorageClass", json!("S3_STANDARD")),
            ("ObjectACL", json!("private")),
            ("Squash", json!("RootSquash")),
            ("ReadOnly", json!(false)),
            ("GuessMIMETypeEnabled", json!(true)),
            ("RequesterPays", json!(false)),
            ("KMSEncrypted", json!(false)),
        ];
        for (member, default) in defaulted {
            if supplied.contains_key(*member) {
                continue;
            }
            assert_eq!(
                &described[*member], default,
                "case {case}: {member} must take its documented default, expected \
                 {default:?}, got {:?}",
                described[*member]
            );
        }
        if !supplied.contains_key("NFSFileShareDefaults") {
            assert_eq!(
                described["NFSFileShareDefaults"],
                json!({
                    "FileMode": "0666",
                    "DirectoryMode": "0777",
                    "GroupId": 65534,
                    "OwnerId": 65534,
                }),
                "case {case}: the NFS defaults must be the documented ones, got {:?}",
                described["NFSFileShareDefaults"]
            );
        }
    }

    let distinct: BTreeSet<&String> = arns.iter().collect();
    assert_eq!(
        distinct.len(),
        arns.len(),
        "every create must mint its own share ARN, got {arns:?}"
    );

    // One call describes several shares, and reports each from its own record.
    let three: Vec<String> = arns.iter().take(3).cloned().collect();
    let answer = call(
        &router,
        "DescribeNFSFileShares",
        json!({ "FileShareARNList": three }),
    )
    .await;
    let reported: Vec<String> = answer.body["NFSFileShareInfoList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|info| info["FileShareARN"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        reported, three,
        "DescribeNFSFileShares must report each ARN it was given, got {reported:?}"
    );
}

/// `ClientToken` is the create's idempotency: the same token on the same gateway answers
/// with the share it already created, and no second share exists. A different token, or
/// the same token on another gateway, is a different share.
#[tokio::test]
async fn create_nfs_file_share_is_idempotent_per_client_token_and_gateway() {
    let router = emulator();
    let first_gateway = activate_file_gateway(&router, "idempotent-gw-1").await;
    let second_gateway = activate_file_gateway(&router, "idempotent-gw-2").await;

    let request = minimal_share(&first_gateway, "token-repeated-01");
    let original = create_share(&router, request.clone()).await;

    for attempt in 1..=3 {
        let repeated = create_share(&router, request.clone()).await;
        assert_eq!(
            repeated, original,
            "attempt {attempt}: a repeat of the same ClientToken must answer with the share \
             it already created, got {repeated:?} for {original:?}"
        );
    }

    let other_token = create_share(
        &router,
        minimal_share(&first_gateway, "token-different-1"),
    )
    .await;
    assert_ne!(
        other_token, original,
        "a different ClientToken must create its own share, got {other_token:?}"
    );

    let other_gateway = create_share(
        &router,
        minimal_share(&second_gateway, "token-repeated-01"),
    )
    .await;
    assert_ne!(
        other_gateway, original,
        "the same token on another gateway must create its own share, got {other_gateway:?}"
    );

    let listed = listed_share_arns(&list_shares(&router, json!({})).await);
    assert_eq!(
        listed.len(),
        3,
        "exactly the three distinct creates may be stored, got {listed:?}"
    );
}

/// An ARN that names no stored record is a refused request — HTTP 400 with the one
/// modelled client error — never a 200 with empty fields and never a 501. The rule holds
/// for every operation that takes an ARN, whichever resource it names.
#[tokio::test]
async fn an_arn_naming_no_stored_record_is_refused() {
    let router = emulator();
    let gateway = activate_file_gateway(&router, "refusal-gw").await;
    let share = create_share(&router, minimal_share(&gateway, "token-refusal-01")).await;

    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "describing a gateway that was never activated",
            "DescribeGatewayInformation",
            json!({ "GatewayARN": NEVER_ACTIVATED_GATEWAY }),
        ),
        (
            "deleting a gateway that was never activated",
            "DeleteGateway",
            json!({ "GatewayARN": NEVER_ACTIVATED_GATEWAY }),
        ),
        (
            "creating a share on a gateway that was never activated",
            "CreateNFSFileShare",
            minimal_share(NEVER_ACTIVATED_GATEWAY, "token-unknown-gw"),
        ),
        (
            "listing the shares of a gateway that was never activated",
            "ListFileShares",
            json!({ "GatewayARN": NEVER_ACTIVATED_GATEWAY }),
        ),
        (
            "describing a share that was never created",
            "DescribeNFSFileShares",
            json!({ "FileShareARNList": [NEVER_CREATED_SHARE] }),
        ),
        (
            "describing a known share beside one that was never created",
            "DescribeNFSFileShares",
            json!({ "FileShareARNList": [&share, NEVER_CREATED_SHARE] }),
        ),
        (
            "deleting a share that was never created",
            "DeleteFileShare",
            json!({ "FileShareARN": NEVER_CREATED_SHARE }),
        ),
    ];

    for (case, operation, request) in cases {
        let answer = call(&router, operation, request).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: {operation} must answer HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            INVALID_GATEWAY_REQUEST,
            "case {case}: {operation} must answer the modelled client error, got {:?}",
            answer.body
        );
    }

    // The refusals changed nothing: the gateway and its one share are still there.
    assert_eq!(
        all_listed_gateways(&router).await,
        vec![gateway.clone()],
        "a refused request must leave the stored gateways alone"
    );
    assert_eq!(
        listed_share_arns(&list_shares(&router, json!({})).await),
        vec![share],
        "a refused request must leave the stored shares alone"
    );
}

/// An NFS file share lives only on an S3 File Gateway ("This operation is only supported
/// for S3 File Gateways"), and the encryption members must agree ("if EncryptionType is
/// SseS3, then KMSEncrypted must be false. If EncryptionType is SseKms or DsseKms, then
/// KMSEncrypted must be true", and `KMSKey` "must be set if KMSEncrypted is true").
#[tokio::test]
async fn create_nfs_file_share_refuses_a_request_the_documentation_rules_out() {
    let router = emulator();
    let file_gateway = activate_file_gateway(&router, "s3-file-gw").await;

    // Every other gateway type is a gateway an NFS share cannot be created on.
    for gateway_type in ["CACHED", "STORED", "VTL", "FILE_FSX_SMB"] {
        let mut activation = minimal_activation(&format!("{gateway_type}-gw"));
        activation["GatewayType"] = json!(gateway_type);
        let activated = call(&router, "ActivateGateway", activation).await;
        let arn = activated.string("GatewayARN");

        let answer = call(
            &router,
            "CreateNFSFileShare",
            minimal_share(&arn, &format!("token-{gateway_type}")),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {gateway_type}: an NFS share is only supported on an S3 File Gateway, got \
             {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            INVALID_GATEWAY_REQUEST,
            "case {gateway_type}: the refusal carries the modelled client error, got {:?}",
            answer.body
        );
    }

    const KMS_KEY: &str =
        "arn:aws:kms:eu-west-1:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab";

    /// One encryption row: which of the three encryption members the request carries, and
    /// whether the three can hold together.
    struct Encryption {
        case: &'static str,
        encryption_type: Option<&'static str>,
        kms_encrypted: Option<bool>,
        kms_key: Option<&'static str>,
        accepted: bool,
    }

    let row = |case, encryption_type, kms_encrypted, kms_key, accepted| Encryption {
        case,
        encryption_type,
        kms_encrypted,
        kms_key,
        accepted,
    };
    let cases = vec![
        row("no encryption members at all", None, None, None, true),
        row("S3-managed keys, said once", Some("SseS3"), None, None, true),
        row("S3-managed keys, said twice and agreeing", Some("SseS3"), Some(false), None, true),
        row("KMS keys, agreeing, with a key", Some("SseKms"), Some(true), Some(KMS_KEY), true),
        row("dual-layer KMS, with a key", Some("DsseKms"), Some(true), Some(KMS_KEY), true),
        row("KMS keys named only by EncryptionType", Some("SseKms"), None, Some(KMS_KEY), true),
        row("the deprecated flag alone, with a key", None, Some(true), Some(KMS_KEY), true),
        row("S3-managed keys contradicted by the flag", Some("SseS3"), Some(true), Some(KMS_KEY), false),
        row("KMS keys contradicted by the flag", Some("SseKms"), Some(false), Some(KMS_KEY), false),
        row("dual-layer KMS contradicted by the flag", Some("DsseKms"), Some(false), Some(KMS_KEY), false),
        row("KMS keys with no key to use", Some("SseKms"), Some(true), None, false),
        row("the deprecated flag with no key to use", None, Some(true), None, false),
    ];

    let mut accepted = 0;
    for (index, Encryption { case, encryption_type, kms_encrypted, kms_key, accepted: ok }) in
        cases.iter().enumerate()
    {
        let mut request = minimal_share(&file_gateway, &format!("token-encryption-{index:02}"));
        if let Some(encryption_type) = encryption_type {
            request["EncryptionType"] = json!(encryption_type);
        }
        if let Some(kms_encrypted) = kms_encrypted {
            request["KMSEncrypted"] = json!(kms_encrypted);
        }
        if let Some(kms_key) = kms_key {
            request["KMSKey"] = json!(kms_key);
        }

        let answer = call(&router, "CreateNFSFileShare", request).await;
        if *ok {
            accepted += 1;
            assert_eq!(
                answer.status, 200,
                "case {case}: the encryption members agree, so the create must succeed, got \
                 {} with {:?}",
                answer.status, answer.body
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case}: the encryption members cannot hold together, so the create must \
                 be refused, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                INVALID_GATEWAY_REQUEST,
                "case {case}: the refusal carries the modelled client error, got {:?}",
                answer.body
            );
        }
    }

    let listed = listed_share_arns(&list_shares(&router, json!({})).await);
    assert_eq!(
        listed.len(),
        accepted,
        "only the accepted rows may be stored: expected {accepted} shares, got {listed:?}"
    );
}

/// `ListFileShares` reports the shares of the gateway it is given, and every share when it
/// is given none; `@paginated(outputToken: NextMarker, pageSize: Limit)`, so following
/// `NextMarker` enumerates each share exactly once, in one stable order, and no page is
/// longer than was asked for.
#[tokio::test]
async fn list_file_shares_filters_by_gateway_and_pages_over_one_stable_order() {
    let router = emulator();
    let first = activate_file_gateway(&router, "paging-gw-1").await;
    let second = activate_file_gateway(&router, "paging-gw-2").await;

    let mut on_first = Vec::new();
    for index in 0..5 {
        on_first.push(create_share(&router, minimal_share(&first, &format!("token-a-{index:04}"))).await);
    }
    let mut on_second = Vec::new();
    for index in 0..2 {
        on_second.push(create_share(&router, minimal_share(&second, &format!("token-b-{index:04}"))).await);
    }
    let everything: Vec<String> = on_first.iter().chain(on_second.iter()).cloned().collect();

    assert_eq!(
        listed_share_arns(&list_shares(&router, json!({ "GatewayARN": &first })).await),
        on_first,
        "ListFileShares must report exactly the shares of the gateway it was given"
    );
    assert_eq!(
        listed_share_arns(&list_shares(&router, json!({ "GatewayARN": &second })).await),
        on_second,
        "ListFileShares must report exactly the shares of the gateway it was given"
    );
    assert_eq!(
        listed_share_arns(&list_shares(&router, json!({})).await),
        everything,
        "with no gateway named, ListFileShares must report every share"
    );

    for limit in [1, 2, 3, 6, 7, 100] {
        let mut walked = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;
        loop {
            let request = match &marker {
                Some(marker) => json!({ "Limit": limit, "Marker": marker }),
                None => json!({ "Limit": limit }),
            };
            let answer = list_shares(&router, request).await;
            let page = listed_share_arns(&answer);
            assert!(
                page.len() <= limit as usize,
                "case Limit={limit}: a page carried {} shares: {page:?}",
                page.len()
            );
            assert_eq!(
                answer.body["Marker"].as_str().map(str::to_string),
                marker,
                "case Limit={limit}: the answer must echo the Marker the request carried, \
                 got {:?}",
                answer.body["Marker"]
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= everything.len() + 1,
                "case Limit={limit}: pagination did not terminate, walked {walked:?}"
            );
            match answer.body["NextMarker"].as_str() {
                Some(next) => marker = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, everything,
            "case Limit={limit}: walking every page must enumerate every share once, in the \
             same order"
        );
    }

    // The same bound, over one gateway's shares only.
    let page = list_shares(&router, json!({ "GatewayARN": &first, "Limit": 2 })).await;
    let arns = listed_share_arns(&page);
    assert_eq!(
        arns,
        on_first[..2].to_vec(),
        "a bounded page of one gateway's shares must be its first Limit shares, got {arns:?}"
    );
    assert_eq!(
        page.body["NextMarker"].as_str(),
        Some(on_first[1].as_str()),
        "more shares remain, so NextMarker must name where to continue, got {:?}",
        page.body["NextMarker"]
    );
}

/// `ListGateways` is `@paginated(inputToken: Marker, outputToken: Marker, pageSize:
/// Limit)`: whatever `Limit` it is given, following `Marker` to the end enumerates every
/// activated gateway exactly once, in one stable order, and no page is longer than asked.
#[tokio::test]
async fn list_gateways_pages_over_one_stable_order() {
    let router = emulator();
    let mut activated = Vec::new();
    for index in 0..7 {
        activated.push(activate_file_gateway(&router, &format!("page-gw-{index}")).await);
    }

    for limit in [1, 2, 3, 6, 7, 100] {
        let mut walked = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;
        loop {
            let request = match &marker {
                Some(marker) => json!({ "Limit": limit, "Marker": marker }),
                None => json!({ "Limit": limit }),
            };
            let answer = call(&router, "ListGateways", request).await;
            assert_eq!(
                answer.status, 200,
                "case Limit={limit}: expected HTTP 200, got {} with {:?}",
                answer.status, answer.body
            );
            let page: Vec<String> = answer.body["Gateways"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|info| info["GatewayARN"].as_str().unwrap_or_default().to_string())
                .collect();
            assert!(
                page.len() <= limit as usize,
                "case Limit={limit}: a page carried {} gateways: {page:?}",
                page.len()
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= activated.len() + 1,
                "case Limit={limit}: pagination did not terminate, walked {walked:?}"
            );
            match answer.body["Marker"].as_str() {
                Some(next) => marker = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, activated,
            "case Limit={limit}: walking every page must enumerate every gateway once, in \
             the same order"
        );
    }
}

/// A share the gateway is still building reports `CREATING`, and the clock — not a call —
/// is what owes it `AVAILABLE`.
#[tokio::test]
async fn a_new_file_share_reports_creating_until_the_clock_owes_it_available() {
    let router = emulator();
    let gateway = activate_file_gateway(&router, "transition-gw").await;

    let started = Instant::now();
    let arn = create_share(&router, minimal_share(&gateway, "token-transition")).await;

    let immediately = describe_share(&router, &arn).await;
    if started.elapsed() < FILE_SHARE_TRANSITION {
        assert_eq!(
            immediately["FileShareStatus"],
            json!("CREATING"),
            "a caller acting inside the transition span still sees CREATING, got {:?}",
            immediately["FileShareStatus"]
        );
        let listed = list_shares(&router, json!({ "GatewayARN": &gateway })).await;
        assert_eq!(
            listed.body["FileShareInfoList"][0]["FileShareStatus"],
            json!("CREATING"),
            "ListFileShares reports the same transient status, got {:?}",
            listed.body["FileShareInfoList"]
        );
    }

    // The transition is owed by the clock, not by a call: nothing is done here but wait.
    tokio::time::sleep(FILE_SHARE_TRANSITION + Duration::from_millis(200)).await;

    let settled = describe_share(&router, &arn).await;
    assert_eq!(
        settled["FileShareStatus"],
        json!("AVAILABLE"),
        "once the transition span has passed the share is AVAILABLE, got {:?}",
        settled["FileShareStatus"]
    );
    let listed = list_shares(&router, json!({ "GatewayARN": &gateway })).await;
    assert_eq!(
        listed.body["FileShareInfoList"][0],
        json!({
            "FileShareType": "NFS",
            "FileShareARN": &arn,
            "FileShareId": settled["FileShareId"],
            "FileShareStatus": "AVAILABLE",
            "GatewayARN": &gateway,
        }),
        "ListFileShares projects the settled record, got {:?}",
        listed.body["FileShareInfoList"][0]
    );
}

/// Deleting removes state: a deleted share is gone from every report, a deleted gateway is
/// gone and takes its shares with it, and a second delete of either is refused.
#[tokio::test]
async fn deleting_removes_the_record_and_a_deleted_gateway_takes_its_shares() {
    let router = emulator();
    let kept = activate_file_gateway(&router, "kept-gw").await;
    let doomed = activate_file_gateway(&router, "doomed-gw").await;

    let kept_share = create_share(&router, minimal_share(&kept, "token-kept-0001")).await;
    let deleted_share = create_share(&router, minimal_share(&kept, "token-deleted-01")).await;
    let doomed_shares = vec![
        create_share(&router, minimal_share(&doomed, "token-doomed-001")).await,
        create_share(&router, minimal_share(&doomed, "token-doomed-002")).await,
    ];

    // DeleteFileShare: the share leaves the store, and nothing else does.
    let deleted = call(
        &router,
        "DeleteFileShare",
        json!({ "FileShareARN": &deleted_share }),
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "expected HTTP 200 from DeleteFileShare, got {} with {:?}",
        deleted.status, deleted.body
    );
    assert_eq!(
        deleted.string("FileShareARN"),
        deleted_share,
        "DeleteFileShare must answer with the ARN it deleted, got {:?}",
        deleted.body
    );
    assert_eq!(
        listed_share_arns(&list_shares(&router, json!({ "GatewayARN": &kept })).await),
        vec![kept_share.clone()],
        "a deleted share must be gone from ListFileShares"
    );
    let gone = call(
        &router,
        "DescribeNFSFileShares",
        json!({ "FileShareARNList": [&deleted_share] }),
    )
    .await;
    assert_eq!(
        gone.status, 400,
        "a deleted share must be gone from DescribeNFSFileShares, got {} with {:?}",
        gone.status, gone.body
    );
    let twice = call(
        &router,
        "DeleteFileShare",
        json!({ "FileShareARN": &deleted_share }),
    )
    .await;
    assert_eq!(
        twice.status, 400,
        "deleting a share twice must be refused, got {} with {:?}",
        twice.status, twice.body
    );

    // The deleted share's ClientToken is forgotten with it, so the same token creates a
    // new share rather than answering with the removed one.
    let recreated = create_share(&router, minimal_share(&kept, "token-deleted-01")).await;
    assert_ne!(
        recreated, deleted_share,
        "a token whose share was deleted must create a new share, got {recreated:?}"
    );

    // DeleteGateway: the gateway leaves the store, and so do its shares.
    let removed = call(&router, "DeleteGateway", json!({ "GatewayARN": &doomed })).await;
    assert_eq!(
        removed.status, 200,
        "expected HTTP 200 from DeleteGateway, got {} with {:?}",
        removed.status, removed.body
    );
    assert_eq!(
        removed.string("GatewayARN"),
        doomed,
        "DeleteGateway must answer with the ARN it deleted, got {:?}",
        removed.body
    );
    assert_eq!(
        all_listed_gateways(&router).await,
        vec![kept.clone()],
        "a deleted gateway must be gone from ListGateways"
    );
    let described = call(
        &router,
        "DescribeGatewayInformation",
        json!({ "GatewayARN": &doomed }),
    )
    .await;
    assert_eq!(
        described.status, 400,
        "a deleted gateway must be refused by DescribeGatewayInformation, got {} with {:?}",
        described.status, described.body
    );
    assert_eq!(
        described.error_code(),
        INVALID_GATEWAY_REQUEST,
        "the refusal carries the modelled client error, got {:?}",
        described.body
    );

    let surviving = listed_share_arns(&list_shares(&router, json!({})).await);
    for share in &doomed_shares {
        assert!(
            !surviving.contains(share),
            "deleting a gateway must drop its file shares, {share} survived in {surviving:?}"
        );
    }
    assert_eq!(
        surviving,
        vec![kept_share, recreated],
        "exactly the shares of the surviving gateway may remain, got {surviving:?}"
    );
    let orphaned = call(
        &router,
        "DescribeNFSFileShares",
        json!({ "FileShareARNList": [&doomed_shares[0]] }),
    )
    .await;
    assert_eq!(
        orphaned.status, 400,
        "a share whose gateway was deleted must be gone, got {} with {:?}",
        orphaned.status, orphaned.body
    );
}

/// The store is bounded: a caller cannot grow it without end. Past the gateway ceiling an
/// activation is refused, past either file-share ceiling a create is refused, and a
/// refusal at a ceiling is the modelled client error rather than a silent success — while
/// a retry of a `ClientToken` already recorded still answers, and a gateway with room
/// still accepts a share.
#[tokio::test]
async fn the_store_is_bounded_so_a_caller_cannot_grow_it_without_end() {
    let router = emulator();

    let mut gateways = Vec::new();
    for index in 0..MAX_GATEWAYS {
        gateways.push(activate_file_gateway(&router, &format!("bounded-gw-{index}")).await);
    }

    let mut one_too_many = minimal_activation("one-gateway-too-many");
    one_too_many["GatewayType"] = json!("FILE_S3");
    let refused = call(&router, "ActivateGateway", one_too_many).await;
    assert_eq!(
        refused.status, 400,
        "case gateway ceiling: the {}th activation must be refused, got {} with {:?}",
        MAX_GATEWAYS + 1,
        refused.status,
        refused.body
    );
    assert_eq!(
        refused.error_code(),
        INVALID_GATEWAY_REQUEST,
        "case gateway ceiling: the refusal carries the modelled client error, got {:?}",
        refused.body
    );
    assert_eq!(
        all_listed_gateways(&router).await.len(),
        MAX_GATEWAYS,
        "case gateway ceiling: a refused activation must store nothing"
    );

    // The per-gateway ceiling.
    let mut first_share = String::new();
    for index in 0..MAX_FILE_SHARES_PER_GATEWAY {
        let arn = create_share(
            &router,
            minimal_share(&gateways[0], &format!("tok-a-{index:05}")),
        )
        .await;
        if index == 0 {
            first_share = arn;
        }
    }
    let refused = call(
        &router,
        "CreateNFSFileShare",
        minimal_share(&gateways[0], "tok-a-one-too-many"),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case per-gateway ceiling: the {}th share on one gateway must be refused, got {} \
         with {:?}",
        MAX_FILE_SHARES_PER_GATEWAY + 1,
        refused.status,
        refused.body
    );
    assert_eq!(
        refused.error_code(),
        INVALID_GATEWAY_REQUEST,
        "case per-gateway ceiling: the refusal carries the modelled client error, got {:?}",
        refused.body
    );
    let retried = create_share(&router, minimal_share(&gateways[0], "tok-a-00000")).await;
    assert_eq!(
        retried, first_share,
        "case per-gateway ceiling: a retry of a recorded token must still answer with its \
         share, got {retried:?} for {first_share:?}"
    );
    create_share(&router, minimal_share(&gateways[1], "tok-b-00000")).await;

    // The whole-store ceiling, filled over as many gateways as it takes.
    let mut total = MAX_FILE_SHARES_PER_GATEWAY + 1;
    let mut on_this_gateway = 1;
    let mut gateway = 1;
    while total < MAX_FILE_SHARES {
        if on_this_gateway == MAX_FILE_SHARES_PER_GATEWAY {
            gateway += 1;
            on_this_gateway = 0;
        }
        create_share(
            &router,
            minimal_share(
                &gateways[gateway],
                &format!("tok-{gateway:03}-{on_this_gateway:03}"),
            ),
        )
        .await;
        on_this_gateway += 1;
        total += 1;
    }

    // The last gateway holds no shares at all, so only the whole-store ceiling can
    // refuse this one.
    let refused = call(
        &router,
        "CreateNFSFileShare",
        minimal_share(&gateways[MAX_GATEWAYS - 1], "tok-store-full-1"),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "case store ceiling: the {}th share must be refused even on an empty gateway, got \
         {} with {:?}",
        MAX_FILE_SHARES + 1,
        refused.status,
        refused.body
    );
    assert_eq!(
        refused.error_code(),
        INVALID_GATEWAY_REQUEST,
        "case store ceiling: the refusal carries the modelled client error, got {:?}",
        refused.body
    );

    let stored = all_listed_shares(&router, None).await;
    assert_eq!(
        stored.len(),
        MAX_FILE_SHARES,
        "case store ceiling: a refused create must store nothing, so the store holds exactly \
         its ceiling"
    );

    // No number a request carries sizes a response: a `Limit` above the server's page
    // ceiling is clamped to it, and `NextMarker` still reaches the rest.
    let oversized = list_shares(
        &router,
        json!({ "Limit": i32::try_from(MAX_FILE_SHARES).expect("the ceiling fits an i32") }),
    )
    .await;
    let page = listed_share_arns(&oversized);
    assert_eq!(
        page.len(),
        MAX_PAGE_LIMIT as usize,
        "case page ceiling: a Limit of {MAX_FILE_SHARES} must be clamped to {MAX_PAGE_LIMIT}, \
         got a page of {}",
        page.len()
    );
    assert_eq!(
        oversized.body["NextMarker"].as_str(),
        Some(page[MAX_PAGE_LIMIT as usize - 1].as_str()),
        "case page ceiling: shares remain, so NextMarker must name where to continue, got {:?}",
        oversized.body["NextMarker"]
    );
}
