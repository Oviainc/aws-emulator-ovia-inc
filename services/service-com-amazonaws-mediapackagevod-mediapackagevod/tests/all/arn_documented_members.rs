// SPDX-License-Identifier: CC0-1.0

//! Every input member this model DOCUMENTS as an ARN is read as one.
//!
//! The rule under test, stated once: a request member whose documentation says it is
//! the ARN of a named AWS service (and, where the documentation names the resource
//! kind too, of that kind of resource) is accepted only when its value is a
//! well-formed resource ARN of that service — six `:`-separated fields opening with
//! `arn`, a non-empty partition, vendor and resource — and otherwise the request is
//! refused as a parameter violation (HTTP 422 `UnprocessableEntityException`,
//! attributed to that member) having created and changed nothing.
//!
//! The members the rule covers, from the model
//! (`smithy_query(query="shape",shape="CreateAssetRequest,Authorization,SpekeKeyProvider")`):
//!
//! | member | documented as |
//! |---|---|
//! | `CreateAsset$SourceArn` | "ARN of the source object in S3" |
//! | `CreateAsset$SourceRoleArn` | "The IAM role ARN used to access the source S3 bucket" |
//! | `Authorization$CdnIdentifierSecret` | "the ARN for the secret in AWS Secrets Manager" |
//! | `Authorization$SecretsRoleArn` | "the ARN for the IAM role" |
//! | `SpekeKeyProvider$RoleArn` | "an ARN of an IAM role" |
//!
//! `Authorization` is reachable through `CreatePackagingGroup` and
//! `UpdatePackagingGroup`, and `SpekeKeyProvider` through the encryption block of each
//! of the four packaging formats, so each of those paths is its own row: the rule is
//! about the member, not about the operation that happens to carry it.
//!
//! Every case drives real HTTP through the router the production binary builds, over
//! its own engine, so no case observes another's state.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every case signs for; it only has to be a legal credential scope.
const REGION: &str = "us-west-2";

/// The PackagingGroup every case's setup creates: the parent an asset and a packaging
/// configuration need, and the group the `UpdatePackagingGroup` rows address.
const GROUP: &str = "vod-group";

/// The id a create under test uses for the resource it would create. Each case runs
/// on a fresh engine, so one id is enough.
const SUBJECT: &str = "subject";

/// A well-formed S3 object ARN, for the members a case is not varying.
const GOOD_S3_OBJECT: &str = "arn:aws:s3:::test-bucket/asset.smil";

/// A well-formed IAM role ARN, for the members a case is not varying.
const GOOD_IAM_ROLE: &str = "arn:aws:iam::111122223333:role/MediaPackageVod";

/// A well-formed Secrets Manager secret ARN, for the members a case is not varying.
const GOOD_SECRET: &str = "arn:aws:secretsmanager:us-west-2:111122223333:secret:cdn-AbCdEf";

/// The modelled refusal of an invalid request parameter: error code and `httpError`
/// (`smithy_query(query="shape",shape="UnprocessableEntityException")` — `errorKind
/// client`, `httpError 422`).
const UNPROCESSABLE: (&str, u16) = ("UnprocessableEntityException", 422);

/// What one answer says about itself: the HTTP status, the error shape
/// `x-amzn-errortype` names, and the parsed body.
struct Answer {
    status: u16,
    errortype: String,
    body: Value,
}

impl Answer {
    /// The `message` member, or `"<unset>"` when the body carries none.
    fn message(&self) -> String {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("<unset>")
            .to_string()
    }

    /// The per-attribute `errors` map, or `Value::Null` when the body carries none.
    fn errors(&self) -> Value {
        self.body.get("errors").cloned().unwrap_or(Value::Null)
    }

    /// The error shape the body itself names (restJson1's `__type`).
    fn json_type(&self) -> String {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or("<unset>")
            .to_string()
    }
}

/// A router carrying exactly this service over one shared engine.
fn router() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_mediapackagevod_mediapackagevod::EmulatedService::new(Arc::new(
            engine::InMemoryEngine::new(),
        )),
    );
    router
}

/// Send one request and report what it answered.
async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: Option<Value>) -> Answer {
    let host = format!("mediapackage-vod.{REGION}.amazonaws.com");
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host.clone())
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}\
                 /mediapackage-vod/aws4_request, SignedHeaders=host, Signature=unverified"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed test request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let errortype = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .unwrap_or_else(|| "<unset>".to_string());
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        errortype,
        body,
    }
}

/// The value at a dotted WIRE member path (`authorization.secretsRoleArn`), or
/// `Value::Null` when the body carries nothing there — so a projection that dropped
/// the member is a visible failure rather than a panic.
fn at_path(body: &Value, path: &str) -> Value {
    let mut node = body;
    for segment in path.split('.') {
        match node.get(segment) {
            Some(next) => node = next,
            None => return Value::Null,
        }
    }
    node.clone()
}

/// Which kind of resource an ARN member must name, which decides the values that are
/// rows for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArnKind {
    /// An object in Amazon S3 — vendor `s3`; the resource field opens with the bucket
    /// name, so no resource type is fixed.
    S3Object,
    /// An IAM role — vendor `iam`, resource type `role`.
    IamRole,
    /// A secret in AWS Secrets Manager — vendor `secretsmanager`, resource type
    /// `secret`.
    SecretsManagerSecret,
}

impl ArnKind {
    /// The constraint phrase the refusal of a member of this kind carries.
    fn phrase(self) -> &'static str {
        match self {
            Self::S3Object => "must be the ARN of an object in Amazon S3",
            Self::IamRole => "must be the ARN of an IAM role",
            Self::SecretsManagerSecret => "must be the ARN of a secret in AWS Secrets Manager",
        }
    }

    /// ARNs of this kind that the rule ADMITS: they differ in partition, Region,
    /// account, resource-name form and length, because the rule is about the ARN's
    /// vendor and resource type and not about the rest of the value.
    fn accepted(self) -> Vec<(&'static str, String)> {
        match self {
            Self::S3Object => vec![
                ("a bucket and key", GOOD_S3_OBJECT.to_string()),
                (
                    "a nested key",
                    "arn:aws:s3:::media-source/vod/2026/01/title.smil".to_string(),
                ),
                (
                    "another partition",
                    "arn:aws-cn:s3:::bucket-cn/o.m3u8".to_string(),
                ),
                (
                    "a long key",
                    format!("arn:aws:s3:::b/{}.smil", "k".repeat(200)),
                ),
            ],
            Self::IamRole => vec![
                ("a role", GOOD_IAM_ROLE.to_string()),
                (
                    "a role under a path",
                    "arn:aws:iam::444455556666:role/service-role/Vod_Ingest-1".to_string(),
                ),
                (
                    "a one-character name in another partition",
                    "arn:aws-us-gov:iam::777788889999:role/r".to_string(),
                ),
                (
                    "a long name",
                    format!("arn:aws:iam::111122223333:role/{}", "R".repeat(200)),
                ),
            ],
            Self::SecretsManagerSecret => vec![
                ("a secret", GOOD_SECRET.to_string()),
                (
                    "a slashed secret name in another Region",
                    "arn:aws:secretsmanager:eu-central-1:444455556666:secret:team/cdn/auth-0"
                        .to_string(),
                ),
                (
                    "a one-character name in another partition",
                    "arn:aws-cn:secretsmanager:cn-north-1:111122223333:secret:s".to_string(),
                ),
                (
                    "a long name",
                    format!(
                        "arn:aws:secretsmanager:us-west-2:111122223333:secret:{}",
                        "s".repeat(200)
                    ),
                ),
            ],
        }
    }

    /// Values of this kind the rule REFUSES: everything that is not an ARN at all,
    /// plus an ARN of another vendor and — for a kind whose resource type is fixed —
    /// an ARN of that vendor naming another resource type.
    fn refused(self) -> Vec<(&'static str, String)> {
        // Not an ARN at all. These rows hold for every kind: each breaks one of the
        // five things that make a value a resource ARN (all six fields present, the
        // first literally `arn`, a non-empty partition, vendor and resource).
        let mut rows: Vec<(&'static str, String)> = vec![
            ("the empty string", String::new()),
            ("arbitrary text", "not-an-arn".to_string()),
            ("a single character", "a".to_string()),
            (
                "a value whose first field is not arn",
                "urn:aws:iam::111122223333:role/MediaPackageVod".to_string(),
            ),
            (
                "an ARN that stops before its resource",
                "arn:aws:iam::111122223333".to_string(),
            ),
            (
                "an ARN with an empty resource",
                "arn:aws:iam::111122223333:".to_string(),
            ),
            (
                "an ARN with an empty vendor",
                "arn:aws::us-west-2:111122223333:role/MediaPackageVod".to_string(),
            ),
            (
                "an ARN with an empty partition",
                "arn::iam::111122223333:role/MediaPackageVod".to_string(),
            ),
            (
                "the resource alone",
                "role/MediaPackageVod".to_string(),
            ),
        ];
        // An ARN of the wrong vendor, and of the wrong resource type within the right
        // vendor: the two cases the issue's "ARN of the wrong service" names.
        rows.extend(match self {
            Self::S3Object => vec![
                ("an IAM role ARN", GOOD_IAM_ROLE.to_string()),
                ("a Secrets Manager secret ARN", GOOD_SECRET.to_string()),
                (
                    "the vendor in another case",
                    "arn:aws:S3:::test-bucket/asset.smil".to_string(),
                ),
                (
                    "another vendor whose name starts with the expected one",
                    "arn:aws:s3-outposts:us-west-2:111122223333:outpost/op-1/bucket/b".to_string(),
                ),
            ],
            Self::IamRole => vec![
                ("an S3 object ARN", GOOD_S3_OBJECT.to_string()),
                ("a Secrets Manager secret ARN", GOOD_SECRET.to_string()),
                (
                    "an IAM user rather than a role",
                    "arn:aws:iam::111122223333:user/Bob".to_string(),
                ),
                (
                    "an IAM resource type that only prefixes role",
                    "arn:aws:iam::111122223333:role-policy/MediaPackageVod".to_string(),
                ),
                (
                    "the vendor in another case",
                    "arn:aws:IAM::111122223333:role/MediaPackageVod".to_string(),
                ),
            ],
            Self::SecretsManagerSecret => vec![
                ("an IAM role ARN", GOOD_IAM_ROLE.to_string()),
                ("an S3 object ARN", GOOD_S3_OBJECT.to_string()),
                (
                    "a Secrets Manager resource of another type",
                    "arn:aws:secretsmanager:us-west-2:111122223333:secret-version:v1".to_string(),
                ),
                (
                    "the vendor in another case",
                    "arn:aws:SecretsManager:us-west-2:111122223333:secret:cdn".to_string(),
                ),
            ],
        });
        rows
    }
}

/// What the probe must see after a refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AfterRefusal {
    /// The create was refused, so the resource it would have created does not exist.
    NothingCreated,
    /// The update was refused, so the addressed group is still there and still
    /// reports the authorization it had — none.
    GroupUnchanged,
}

/// One member the model documents as an ARN, as a case can drive it end to end.
struct ArnMember {
    /// The operation that carries the member, for the case label.
    operation: &'static str,
    /// That operation's HTTP method.
    method: &'static str,
    /// The resource id the request names.
    id: &'static str,
    /// The request URI, given [`Self::id`].
    uri: fn(&str) -> String,
    /// The request body, given [`Self::id`] and the value under test; every OTHER
    /// member it carries is well formed, so a refusal can only be about this one.
    body: fn(&str, &str) -> Value,
    /// The member's WIRE path, which the `errors` map must be keyed by and which the
    /// member reads back at in a projection.
    member: &'static str,
    /// The same member as the refusal's `message` spells it (snake case).
    message_member: &'static str,
    /// What the member must name.
    kind: ArnKind,
    /// The status the operation answers on success
    /// (`CreatePackagingConfiguration` is modelled 202, the others 200).
    success: u16,
    /// Where to read the resource back.
    probe: fn(&str) -> String,
    /// What that probe must see after a refusal.
    after: AfterRefusal,
}

/// A SPEKE key provider block carrying `role_arn`. `SystemIds` and `Url` are required
/// members of the shape, so every block carries them; the system id is Widevine's
/// published DRM system id.
fn speke(role_arn: &str) -> Value {
    json!({
        "roleArn": role_arn,
        "systemIds": ["edef8ba9-79d6-4ace-a3c8-27dcd51d21ed"],
        "url": "https://speke.example.com/v1/copyProtection",
    })
}

/// A `CreateAsset` body whose `SourceArn` is the value under test.
fn asset_source_arn(id: &str, value: &str) -> Value {
    json!({
        "id": id,
        "packagingGroupId": GROUP,
        "sourceArn": value,
        "sourceRoleArn": GOOD_IAM_ROLE,
    })
}

/// A `CreateAsset` body whose `SourceRoleArn` is the value under test.
fn asset_source_role_arn(id: &str, value: &str) -> Value {
    json!({
        "id": id,
        "packagingGroupId": GROUP,
        "sourceArn": GOOD_S3_OBJECT,
        "sourceRoleArn": value,
    })
}

/// A `CreatePackagingGroup` body whose `Authorization$CdnIdentifierSecret` is the
/// value under test.
fn group_cdn_identifier_secret(id: &str, value: &str) -> Value {
    json!({
        "id": id,
        "authorization": { "cdnIdentifierSecret": value, "secretsRoleArn": GOOD_IAM_ROLE },
    })
}

/// A `CreatePackagingGroup` body whose `Authorization$SecretsRoleArn` is the value
/// under test.
fn group_secrets_role_arn(id: &str, value: &str) -> Value {
    json!({
        "id": id,
        "authorization": { "cdnIdentifierSecret": GOOD_SECRET, "secretsRoleArn": value },
    })
}

/// An `UpdatePackagingGroup` body whose `Authorization$CdnIdentifierSecret` is the
/// value under test (the group is addressed by the URI).
fn update_cdn_identifier_secret(_id: &str, value: &str) -> Value {
    json!({ "authorization": { "cdnIdentifierSecret": value, "secretsRoleArn": GOOD_IAM_ROLE } })
}

/// An `UpdatePackagingGroup` body whose `Authorization$SecretsRoleArn` is the value
/// under test.
fn update_secrets_role_arn(_id: &str, value: &str) -> Value {
    json!({ "authorization": { "cdnIdentifierSecret": GOOD_SECRET, "secretsRoleArn": value } })
}

/// A `CreatePackagingConfiguration` body carrying `block` as its one package block.
fn config_body(id: &str, block: &str, settings: Value) -> Value {
    json!({ "id": id, "packagingGroupId": GROUP, block: settings })
}

/// `CreatePackagingConfiguration` with an HLS package whose SPEKE role is under test.
fn config_hls_role_arn(id: &str, value: &str) -> Value {
    config_body(
        id,
        "hlsPackage",
        json!({
            "hlsManifests": [{}],
            "encryption": { "spekeKeyProvider": speke(value) },
        }),
    )
}

/// `CreatePackagingConfiguration` with a DASH package whose SPEKE role is under test.
fn config_dash_role_arn(id: &str, value: &str) -> Value {
    config_body(
        id,
        "dashPackage",
        json!({
            "dashManifests": [{}],
            "encryption": { "spekeKeyProvider": speke(value) },
        }),
    )
}

/// `CreatePackagingConfiguration` with a CMAF package whose SPEKE role is under test.
fn config_cmaf_role_arn(id: &str, value: &str) -> Value {
    config_body(
        id,
        "cmafPackage",
        json!({
            "hlsManifests": [{}],
            "encryption": { "spekeKeyProvider": speke(value) },
        }),
    )
}

/// `CreatePackagingConfiguration` with an MSS package whose SPEKE role is under test.
fn config_mss_role_arn(id: &str, value: &str) -> Value {
    config_body(
        id,
        "mssPackage",
        json!({
            "mssManifests": [{}],
            "encryption": { "spekeKeyProvider": speke(value) },
        }),
    )
}

/// Every ARN-documented member of this service, one row per path that reaches it.
fn arn_members() -> Vec<ArnMember> {
    vec![
        ArnMember {
            operation: "CreateAsset",
            method: "POST",
            id: SUBJECT,
            uri: |_| "/assets".to_string(),
            body: asset_source_arn,
            member: "sourceArn",
            message_member: "source_arn",
            kind: ArnKind::S3Object,
            success: 200,
            probe: |id| format!("/assets/{id}"),
            after: AfterRefusal::NothingCreated,
        },
        ArnMember {
            operation: "CreateAsset",
            method: "POST",
            id: SUBJECT,
            uri: |_| "/assets".to_string(),
            body: asset_source_role_arn,
            member: "sourceRoleArn",
            message_member: "source_role_arn",
            kind: ArnKind::IamRole,
            success: 200,
            probe: |id| format!("/assets/{id}"),
            after: AfterRefusal::NothingCreated,
        },
        ArnMember {
            operation: "CreatePackagingGroup",
            method: "POST",
            id: SUBJECT,
            uri: |_| "/packaging_groups".to_string(),
            body: group_cdn_identifier_secret,
            member: "authorization.cdnIdentifierSecret",
            message_member: "authorization.cdn_identifier_secret",
            kind: ArnKind::SecretsManagerSecret,
            success: 200,
            probe: |id| format!("/packaging_groups/{id}"),
            after: AfterRefusal::NothingCreated,
        },
        ArnMember {
            operation: "CreatePackagingGroup",
            method: "POST",
            id: SUBJECT,
            uri: |_| "/packaging_groups".to_string(),
            body: group_secrets_role_arn,
            member: "authorization.secretsRoleArn",
            message_member: "authorization.secrets_role_arn",
            kind: ArnKind::IamRole,
            success: 200,
            probe: |id| format!("/packaging_groups/{id}"),
            after: AfterRefusal::NothingCreated,
        },
        ArnMember {
            operation: "UpdatePackagingGroup",
            method: "PUT",
            id: GROUP,
            uri: |id| format!("/packaging_groups/{id}"),
            body: update_cdn_identifier_secret,
            member: "authorization.cdnIdentifierSecret",
            message_member: "authorization.cdn_identifier_secret",
            kind: ArnKind::SecretsManagerSecret,
            success: 200,
            probe: |id| format!("/packaging_groups/{id}"),
            after: AfterRefusal::GroupUnchanged,
        },
        ArnMember {
            operation: "UpdatePackagingGroup",
            method: "PUT",
            id: GROUP,
            uri: |id| format!("/packaging_groups/{id}"),
            body: update_secrets_role_arn,
            member: "authorization.secretsRoleArn",
            message_member: "authorization.secrets_role_arn",
            kind: ArnKind::IamRole,
            success: 200,
            probe: |id| format!("/packaging_groups/{id}"),
            after: AfterRefusal::GroupUnchanged,
        },
        ArnMember {
            operation: "CreatePackagingConfiguration (hlsPackage)",
            method: "POST",
            id: SUBJECT,
            uri: |_| "/packaging_configurations".to_string(),
            body: config_hls_role_arn,
            member: "hlsPackage.encryption.spekeKeyProvider.roleArn",
            message_member: "hls_package.encryption.speke_key_provider.role_arn",
            kind: ArnKind::IamRole,
            success: 202,
            probe: |id| format!("/packaging_configurations/{id}"),
            after: AfterRefusal::NothingCreated,
        },
        ArnMember {
            operation: "CreatePackagingConfiguration (dashPackage)",
            method: "POST",
            id: SUBJECT,
            uri: |_| "/packaging_configurations".to_string(),
            body: config_dash_role_arn,
            member: "dashPackage.encryption.spekeKeyProvider.roleArn",
            message_member: "dash_package.encryption.speke_key_provider.role_arn",
            kind: ArnKind::IamRole,
            success: 202,
            probe: |id| format!("/packaging_configurations/{id}"),
            after: AfterRefusal::NothingCreated,
        },
        ArnMember {
            operation: "CreatePackagingConfiguration (cmafPackage)",
            method: "POST",
            id: SUBJECT,
            uri: |_| "/packaging_configurations".to_string(),
            body: config_cmaf_role_arn,
            member: "cmafPackage.encryption.spekeKeyProvider.roleArn",
            message_member: "cmaf_package.encryption.speke_key_provider.role_arn",
            kind: ArnKind::IamRole,
            success: 202,
            probe: |id| format!("/packaging_configurations/{id}"),
            after: AfterRefusal::NothingCreated,
        },
        ArnMember {
            operation: "CreatePackagingConfiguration (mssPackage)",
            method: "POST",
            id: SUBJECT,
            uri: |_| "/packaging_configurations".to_string(),
            body: config_mss_role_arn,
            member: "mssPackage.encryption.spekeKeyProvider.roleArn",
            message_member: "mss_package.encryption.speke_key_provider.role_arn",
            kind: ArnKind::IamRole,
            success: 202,
            probe: |id| format!("/packaging_configurations/{id}"),
            after: AfterRefusal::NothingCreated,
        },
    ]
}

/// A router whose store holds [`GROUP`] and nothing else — the parent every case's
/// request needs, created with no CDN authorization of its own.
async fn fixture() -> MultiServiceRouter {
    let router = router();
    let answer = call(
        &router,
        "POST",
        "/packaging_groups",
        Some(json!({ "id": GROUP })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "setup: CreatePackagingGroup {GROUP:?} should succeed, got {} body {}",
        answer.status, answer.body
    );
    router
}

/// A value that is not a well-formed ARN of the documented service is refused — 422,
/// attributed to the member — and the request creates and changes nothing.
#[tokio::test]
async fn a_value_that_is_not_an_arn_of_the_documented_service_is_refused() {
    for member in arn_members() {
        for (value_case, value) in member.kind.refused() {
            let case = format!("{} {} = {value_case}", member.operation, member.member);
            let router = fixture().await;

            let answer = call(
                &router,
                member.method,
                &(member.uri)(member.id),
                Some((member.body)(member.id, &value)),
            )
            .await;

            let (code, status) = UNPROCESSABLE;
            assert_eq!(
                answer.status, status,
                "case {case:?}: expected HTTP {status}, got {} body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.errortype, code,
                "case {case:?}: expected x-amzn-errortype {code}, got {:?}",
                answer.errortype
            );
            assert_eq!(
                answer.json_type(),
                code,
                "case {case:?}: the body's __type must name the same shape as the header, got {:?}",
                answer.json_type()
            );
            assert_eq!(
                answer.errors(),
                json!({ member.member: [member.kind.phrase()] }),
                "case {case:?}: the refusal must attribute itself to the offending member, got {}",
                answer.errors()
            );
            assert_eq!(
                answer.message(),
                format!("{} {}", member.message_member, member.kind.phrase()),
                "case {case:?}: expected the member-and-constraint message, got {:?}",
                answer.message()
            );

            // Nothing was persisted: a refused create leaves no resource behind, and a
            // refused update leaves the addressed group exactly as it was.
            let probe = call(&router, "GET", &(member.probe)(member.id), None).await;
            match member.after {
                AfterRefusal::NothingCreated => assert_eq!(
                    probe.status, 404,
                    "case {case:?}: the refused create must persist nothing, got {} body {}",
                    probe.status, probe.body
                ),
                AfterRefusal::GroupUnchanged => {
                    assert_eq!(
                        probe.status, 200,
                        "case {case:?}: the addressed group must survive, got {} body {}",
                        probe.status, probe.body
                    );
                    assert_eq!(
                        probe.body.get("authorization").cloned(),
                        None,
                        "case {case:?}: the refused update must not store an authorization, got {}",
                        probe.body
                    );
                }
            }
        }
    }
}

/// An ARN of the documented service is accepted whatever its partition, Region,
/// account and resource name, and the value is stored verbatim: a later read reports
/// exactly what was sent.
#[tokio::test]
async fn an_arn_of_the_documented_service_is_accepted_and_read_back_verbatim() {
    for member in arn_members() {
        for (value_case, value) in member.kind.accepted() {
            let case = format!("{} {} = {value_case}", member.operation, member.member);
            let router = fixture().await;

            let answer = call(
                &router,
                member.method,
                &(member.uri)(member.id),
                Some((member.body)(member.id, &value)),
            )
            .await;
            assert_eq!(
                answer.status,
                member.success,
                "case {case:?}: expected HTTP {}, got {} body {}",
                member.success,
                answer.status,
                answer.body
            );
            assert_eq!(
                at_path(&answer.body, member.member),
                json!(value),
                "case {case:?}: the answer must report the value that was sent, got {}",
                at_path(&answer.body, member.member)
            );

            // Round trip: the stored record reports the same value on the next read.
            let probe = call(&router, "GET", &(member.probe)(member.id), None).await;
            assert_eq!(
                probe.status, 200,
                "case {case:?}: the resource must read back, got {} body {}",
                probe.status, probe.body
            );
            assert_eq!(
                at_path(&probe.body, member.member),
                json!(value),
                "case {case:?}: the read must report the stored value, got {}",
                at_path(&probe.body, member.member)
            );
        }
    }
}

/// A request whose ARN members are BOTH invalid is told about both in one refusal:
/// the `errors` map carries an entry per offending member, and the message states each
/// attribution in turn.
#[tokio::test]
async fn two_invalid_arn_members_are_reported_in_one_refusal() {
    // (case, the CreateAsset body, the errors map it must answer with, the message)
    let cases: Vec<(&str, Value, Value, String)> = vec![
        (
            "both members arbitrary text",
            json!({
                "id": SUBJECT,
                "packagingGroupId": GROUP,
                "sourceArn": "not-an-arn",
                "sourceRoleArn": "also-not-an-arn",
            }),
            json!({
                "sourceArn": [ArnKind::S3Object.phrase()],
                "sourceRoleArn": [ArnKind::IamRole.phrase()],
            }),
            format!(
                "{} {}, {} {}",
                "source_arn",
                ArnKind::S3Object.phrase(),
                "source_role_arn",
                ArnKind::IamRole.phrase()
            ),
        ),
        (
            "both members empty",
            json!({
                "id": SUBJECT,
                "packagingGroupId": GROUP,
                "sourceArn": "",
                "sourceRoleArn": "",
            }),
            json!({
                "sourceArn": [ArnKind::S3Object.phrase()],
                "sourceRoleArn": [ArnKind::IamRole.phrase()],
            }),
            format!(
                "{} {}, {} {}",
                "source_arn",
                ArnKind::S3Object.phrase(),
                "source_role_arn",
                ArnKind::IamRole.phrase()
            ),
        ),
        (
            "the two ARNs swapped",
            json!({
                "id": SUBJECT,
                "packagingGroupId": GROUP,
                "sourceArn": GOOD_IAM_ROLE,
                "sourceRoleArn": GOOD_S3_OBJECT,
            }),
            json!({
                "sourceArn": [ArnKind::S3Object.phrase()],
                "sourceRoleArn": [ArnKind::IamRole.phrase()],
            }),
            format!(
                "{} {}, {} {}",
                "source_arn",
                ArnKind::S3Object.phrase(),
                "source_role_arn",
                ArnKind::IamRole.phrase()
            ),
        ),
    ];

    for (case, body, errors, message) in cases {
        let router = fixture().await;
        let answer = call(&router, "POST", "/assets", Some(body)).await;
        assert_eq!(
            answer.status, UNPROCESSABLE.1,
            "case {case:?}: expected HTTP {}, got {} body {}",
            UNPROCESSABLE.1, answer.status, answer.body
        );
        assert_eq!(
            answer.errors(),
            errors,
            "case {case:?}: both members must be attributed, got {}",
            answer.errors()
        );
        assert_eq!(
            answer.message(),
            message,
            "case {case:?}: the message must state both attributions, got {:?}",
            answer.message()
        );

        let probe = call(&router, "GET", &format!("/assets/{SUBJECT}"), None).await;
        assert_eq!(
            probe.status, 404,
            "case {case:?}: the refused create must persist nothing, got {} body {}",
            probe.status, probe.body
        );
    }
}

/// The ARN rule is read on the REQUEST, before the group the body names is resolved
/// and before the id is checked for being free — but after the members the service
/// refuses on their own grounds, so each refusal keeps its own attribution.
#[tokio::test]
async fn the_arn_rule_does_not_displace_the_other_parameter_refusals() {
    let router = fixture().await;
    // An asset that exists, so the duplicate-id rule has something to fire on.
    let created = call(
        &router,
        "POST",
        "/assets",
        Some(asset_source_arn(SUBJECT, GOOD_S3_OBJECT)),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "setup: CreateAsset {SUBJECT:?} should succeed, got {} body {}",
        created.status, created.body
    );

    // (case, body, the errors map the refusal must carry)
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "an empty id is still the id's own violation",
            json!({
                "id": "",
                "packagingGroupId": GROUP,
                "sourceArn": GOOD_S3_OBJECT,
                "sourceRoleArn": GOOD_IAM_ROLE,
            }),
            json!({ "id": ["must not be empty"] }),
        ),
        (
            "an unresolvable packagingGroupId is still that member's violation",
            json!({
                "id": "fresh",
                "packagingGroupId": "absent-group",
                "sourceArn": GOOD_S3_OBJECT,
                "sourceRoleArn": GOOD_IAM_ROLE,
            }),
            json!({ "packagingGroupId": ["does not exist"] }),
        ),
        (
            "a junk ARN beside an unresolvable packagingGroupId is the ARN's violation",
            json!({
                "id": "fresh",
                "packagingGroupId": "absent-group",
                "sourceArn": "not-an-arn",
                "sourceRoleArn": GOOD_IAM_ROLE,
            }),
            json!({ "sourceArn": [ArnKind::S3Object.phrase()] }),
        ),
    ];

    for (case, body, errors) in cases {
        let answer = call(&router, "POST", "/assets", Some(body)).await;
        assert_eq!(
            answer.status, UNPROCESSABLE.1,
            "case {case:?}: expected HTTP {}, got {} body {}",
            UNPROCESSABLE.1, answer.status, answer.body
        );
        assert_eq!(
            answer.errors(),
            errors,
            "case {case:?}: expected the refusal to keep its own attribution, got {}",
            answer.errors()
        );
    }

    // A duplicate id is still a conflict rather than a member violation, and the
    // stored asset is untouched by any of the refusals above.
    let duplicate = call(
        &router,
        "POST",
        "/assets",
        Some(asset_source_arn(SUBJECT, GOOD_S3_OBJECT)),
    )
    .await;
    assert_eq!(
        duplicate.message(),
        format!("Asset with id={SUBJECT} already exists"),
        "a duplicate id keeps the conflict message, got {:?}",
        duplicate.message()
    );
    let probe = call(&router, "GET", &format!("/assets/{SUBJECT}"), None).await;
    assert_eq!(
        probe.status, 200,
        "the stored asset survived every refusal, got {} body {}",
        probe.status, probe.body
    );
}

/// `UpdatePackagingGroup` reads the request's ARN members BEFORE it resolves the group
/// the URI addresses, so a junk ARN is answered 422 even for an id this service holds
/// no group for — while the same request with well-formed ARNs still gets the 404 the
/// addressed resource owes. Either way nothing is created.
#[tokio::test]
async fn an_update_reads_its_arn_members_before_it_resolves_the_addressed_group() {
    // Ids differing in length, separators, case and digits; none is stored.
    let missing = ["nope", "a", "Group_42", "dotted.id-0123456789", &"x".repeat(100)];
    // (case, the authorization block, the expected status, the expected errors map)
    for id in missing {
        let rows: Vec<(&str, Value, u16, Value)> = vec![
            (
                "a junk secret ARN",
                json!({ "cdnIdentifierSecret": "not-an-arn", "secretsRoleArn": GOOD_IAM_ROLE }),
                UNPROCESSABLE.1,
                json!({
                    "authorization.cdnIdentifierSecret":
                        [ArnKind::SecretsManagerSecret.phrase()],
                }),
            ),
            (
                "a junk role ARN",
                json!({ "cdnIdentifierSecret": GOOD_SECRET, "secretsRoleArn": "" }),
                UNPROCESSABLE.1,
                json!({ "authorization.secretsRoleArn": [ArnKind::IamRole.phrase()] }),
            ),
            (
                "well-formed ARNs, so the missing group answers",
                json!({ "cdnIdentifierSecret": GOOD_SECRET, "secretsRoleArn": GOOD_IAM_ROLE }),
                404,
                Value::Null,
            ),
        ];
        for (value_case, authorization, status, errors) in rows {
            let case = format!("UpdatePackagingGroup {id:?} with {value_case}");
            let router = fixture().await;
            let answer = call(
                &router,
                "PUT",
                &format!("/packaging_groups/{id}"),
                Some(json!({ "authorization": authorization })),
            )
            .await;
            assert_eq!(
                answer.status, status,
                "case {case:?}: expected HTTP {status}, got {} body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.errors(),
                errors,
                "case {case:?}: expected errors {errors}, got {}",
                answer.errors()
            );

            // The refusal created nothing: the id still resolves to no group.
            let probe = call(&router, "GET", &format!("/packaging_groups/{id}"), None).await;
            assert_eq!(
                probe.status, 404,
                "case {case:?}: the refused update must not create the group, got {} body {}",
                probe.status, probe.body
            );
        }
    }
}
