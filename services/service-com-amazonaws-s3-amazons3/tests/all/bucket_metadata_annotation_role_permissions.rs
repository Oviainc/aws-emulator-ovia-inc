// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the IAM ROLE an S3 Metadata configuration's ANNOTATION table is
//! filled through: "if you include an `AnnotationTableConfiguration` with an IAM role, the
//! role must have a trust policy that allows the Amazon S3 metadata service to assume it,
//! and a permissions policy that grants the actions needed to read annotations from your
//! bucket" (`CreateBucketMetadataConfiguration`'s own documentation).
//!
//! The rule under test: an annotation table is PROVISIONED only under a role IAM shows the
//! S3 metadata service can assume, and a call that names any other role is served with that
//! table recorded `FAILED` carrying the `ErrorDetails` for the role's permissions — a
//! provisioning outcome a reader reports, not a refusal of the call, exactly where the
//! published failures of the journal and inventory tables leave those two. The same rule
//! judges every call that provisions the table: the create that names a whole configuration
//! and `UpdateBucketMetadataAnnotationTableConfiguration` enabling it on one.
//!
//! A role nothing can be shown about is not held against the table: this emulator refuses
//! only what IAM ANSWERED, so a role of ANOTHER account — whose IAM this emulator is not the
//! one to ask — leaves the table to be created, which is also why every other metadata suite
//! in this crate, which mounts S3 alone, provisions an annotation table under a role no IAM
//! record backs.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which
//! principal the trust document names and in which form (one string or a list), which action
//! it names (the assume, a wildcard over it, another action), which effect it states, whether
//! IAM holds the role at all, and which account the role ARN names. Every call goes over the
//! wire through the router and IAM's roles are made by IAM's OWN `CreateRole`, so what is
//! asserted is HTTP behaviour over the records the owning services wrote.

use std::sync::Arc;
use std::time::Duration;

use service_com_amazonaws_s3_amazons3::state::metadata_annotation_role::{
    ANNOTATION_ROLE_PERMISSIONS_CODE, ANNOTATION_ROLE_PERMISSIONS_MESSAGE,
};
use service_com_amazonaws_s3_amazons3::state::METADATA_TABLE_ACTIVATION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::Answer;

/// The Region every request in this suite is signed for, and so the endpoint it addresses.
const REGION: &str = "us-east-1";

/// The service principal an annotation role's trust policy has to admit for S3 Metadata to
/// fill the table through it. Spelled out here rather than read from the crate, so this suite
/// pins the value.
const METADATA_SERVICE_PRINCIPAL: &str = "metadata.s3.amazonaws.com";

/// The IAM action a trust policy grants for a principal to be able to assume the role.
const ASSUME_ROLE: &str = "sts:AssumeRole";

/// The role name every row's annotation table configuration names.
const ROLE_NAME: &str = "s3-metadata-annotations";

/// An account id that is NOT the one account this emulator serves, so a role ARN carrying it
/// names a role of another account's IAM.
const ANOTHER_ACCOUNT: &str = "123456789012";

/// The `TableStatus` of a table whose creation is under way, and of one in service.
const CREATING: &str = "CREATING";
/// See [`CREATING`].
const ACTIVE: &str = "ACTIVE";
/// See [`CREATING`] — the status of a table that could not be created.
const FAILED: &str = "FAILED";

/// An emulator holding S3 beside the IAM the role resolution asks: the question is put over
/// the emulator's own in-process callback, so IAM has to be mounted for it to answer at all.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Nothing here is signed: the `Authorization` header carries only the credential
        // scope, which is where a service reads the Region and the caller's access key id.
        router.set_request_authentication(false);
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// One S3 request, addressed path-style to the Regional endpoint.
    async fn s3(&self, method: &str, uri: &str, body: &[u8]) -> Answer {
        let host = format!("s3.{REGION}.amazonaws.com");
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{uri}"))
            .header("host", host.clone())
            .header("content-type", "application/xml")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/s3/\
                     aws4_request, SignedHeaders=host, Signature=0000"
                ),
            )
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("the test request must build");
        let response = self.router.dispatch(request).await;
        let (parts, body) = response.into_parts();
        Answer {
            status: parts.status.as_u16(),
            headers: parts.headers,
            body,
        }
    }

    /// One awsQuery call to IAM, as the account's own credential.
    async fn iam(&self, action: &str, params: &[(&str, String)]) -> (u16, String) {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("https://iam.amazonaws.com/")
            .header("host", "iam.amazonaws.com")
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/iam/\
                     aws4_request, SignedHeaders=host, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        (
            response.status().as_u16(),
            String::from_utf8_lossy(response.body()).to_string(),
        )
    }

    /// Create the role [`ROLE_NAME`] through IAM's own `CreateRole`, carrying `trust` as its
    /// `AssumeRolePolicyDocument`.
    async fn create_role(&self, case: &str, trust: &str) {
        let (status, body) = self
            .iam(
                "CreateRole",
                &[
                    ("RoleName", ROLE_NAME.to_string()),
                    ("AssumeRolePolicyDocument", trust.to_string()),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: creating the role must succeed, got {status} with {body}"
        );
    }

    /// `PUT /<bucket>` — the general purpose bucket a configuration is created for.
    async fn create_bucket(&self, case: &str, bucket: &str) {
        let created = self.s3("PUT", &format!("/{bucket}"), b"").await;
        assert_eq!(
            created.status,
            200,
            "case {case:?}: the bucket must be created, got {} with {:?}",
            created.status,
            text(&created)
        );
    }

    /// `POST /<bucket>?metadataConfiguration` — `CreateBucketMetadataConfiguration` with a
    /// journal table and an annotation table in `state`, naming `role` when it names one.
    async fn create_configuration(
        &self,
        bucket: &str,
        state: &str,
        role: Option<&str>,
    ) -> Answer {
        let role = role.map_or_else(String::new, |role| format!("<Role>{role}</Role>"));
        let body = format!(
            "<MetadataConfiguration><JournalTableConfiguration><RecordExpiration>\
             <Expiration>ENABLED</Expiration><Days>7</Days></RecordExpiration>\
             </JournalTableConfiguration><AnnotationTableConfiguration>\
             <ConfigurationState>{state}</ConfigurationState>{role}\
             </AnnotationTableConfiguration></MetadataConfiguration>"
        );
        self.s3(
            "POST",
            &format!("/{bucket}?metadataConfiguration"),
            body.as_bytes(),
        )
        .await
    }

    /// `PUT /<bucket>?metadataAnnotationTable` —
    /// `UpdateBucketMetadataAnnotationTableConfiguration` enabling the annotation table under
    /// `role`.
    async fn enable_annotation_table(&self, bucket: &str, role: &str) -> Answer {
        let body = format!(
            "<AnnotationTableConfiguration>\
             <ConfigurationState>ENABLED</ConfigurationState><Role>{role}</Role>\
             </AnnotationTableConfiguration>"
        );
        self.s3(
            "PUT",
            &format!("/{bucket}?metadataAnnotationTable"),
            body.as_bytes(),
        )
        .await
    }

    /// `GET /<bucket>?metadataConfiguration` — `GetBucketMetadataConfiguration`.
    async fn read_configuration(&self, bucket: &str) -> String {
        let read = self
            .s3("GET", &format!("/{bucket}?metadataConfiguration"), b"")
            .await;
        assert_eq!(
            read.status,
            200,
            "the read of the configuration must answer 200, got {} with {:?}",
            read.status,
            text(&read)
        );
        text(&read)
    }
}

/// The ARN of the role [`ROLE_NAME`] in `account`.
fn role_arn(account: &str) -> String {
    format!("arn:aws:iam::{account}:role/{ROLE_NAME}")
}

/// A trust document with one statement: `effect` for the `Principal.Service` element
/// `service` (written as JSON, so a row may name one string or a list) performing `action`.
fn trust(effect: &str, service: &str, action: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"{effect}\",\
         \"Principal\":{{\"Service\":{service}}},\"Action\":\"{action}\"}}]}}"
    )
}

/// The trust document a role S3 Metadata can fill an annotation table through carries.
fn metadata_trust() -> String {
    trust(
        "Allow",
        &format!("\"{METADATA_SERVICE_PRINCIPAL}\""),
        ASSUME_ROLE,
    )
}

/// The response body as text, for an assertion's message.
fn text(answer: &Answer) -> String {
    String::from_utf8_lossy(&answer.body).to_string()
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC 3986's
/// unreserved characters literal — so a policy document's braces and quotes survive the round
/// trip.
///
/// A copy: an integration-test module cannot be imported across crates, so every suite that
/// drives an awsQuery service holds its own.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// The inner XML of the first `<name>…</name>` container of `fragment`, when it carries one.
fn inside<'a>(fragment: &'a str, name: &str) -> Option<&'a str> {
    let (_, rest) = fragment.split_once(&format!("<{name}>"))?;
    let (inner, _) = rest.split_once(&format!("</{name}>"))?;
    Some(inner)
}

/// The `TableStatus`, `ErrorCode` and `ErrorMessage` the annotation table result of `body`
/// reports, each `None` when the result carries none.
fn annotation_result(body: &str) -> (Option<String>, Option<String>, Option<String>) {
    let Some(fragment) = inside(body, "AnnotationTableConfigurationResult") else {
        return (None, None, None);
    };
    let element = |name: &str| inside(fragment, name).map(super::bucket_tagging::unescaped);
    (
        element("TableStatus"),
        element("ErrorCode"),
        element("ErrorMessage"),
    )
}

/// How long a row waits for a table to fall due: the span the store records plus slack for a
/// loaded machine.
///
/// A blocking sleep rather than `tokio::time::sleep(..).await`: the workspace's tokio carries
/// only the `macros`/`rt` features, so its timer driver is not built.
fn wait_for_the_tables() {
    std::thread::sleep(METADATA_TABLE_ACTIVATION_DELAY + Duration::from_millis(250));
}

/// One row of the role table: what IAM holds for the role, which account the ARN the request
/// carries names, and whether the annotation table is then provisioned.
struct Row {
    case: &'static str,
    /// The trust document IAM holds the role under, or `None` for a row whose role IAM holds
    /// no record of at all.
    trust: Option<String>,
    /// Whether the ARN the request carries names THIS account's role; a row that does not
    /// names another account's, whose IAM this emulator is not the one to ask.
    this_account: bool,
    /// Whether the table is provisioned: `true` is a table that reaches service with no
    /// error, `false` is one recorded `FAILED` carrying the role's own `ErrorDetails`.
    provisioned: bool,
}

/// The role table: the rows differ along what the rule generalises over — which principal the
/// trust document names and in which form, which action and effect it states, whether IAM
/// holds the role at all, and which account the ARN names.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a trust document admitting the S3 metadata service principal",
            trust: Some(metadata_trust()),
            this_account: true,
            provisioned: true,
        },
        Row {
            case: "the metadata principal among a list of services",
            trust: Some(trust(
                "Allow",
                &format!("[\"s3.amazonaws.com\",\"{METADATA_SERVICE_PRINCIPAL}\"]"),
                ASSUME_ROLE,
            )),
            this_account: true,
            provisioned: true,
        },
        Row {
            case: "a wildcard over the assume action",
            trust: Some(trust(
                "Allow",
                &format!("\"{METADATA_SERVICE_PRINCIPAL}\""),
                "sts:*",
            )),
            this_account: true,
            provisioned: true,
        },
        Row {
            case: "a trust document admitting another service principal",
            trust: Some(trust("Allow", "\"s3.amazonaws.com\"", ASSUME_ROLE)),
            this_account: true,
            provisioned: false,
        },
        Row {
            case: "a trust document naming the metadata principal under another action",
            trust: Some(trust(
                "Allow",
                &format!("\"{METADATA_SERVICE_PRINCIPAL}\""),
                "sts:TagSession",
            )),
            this_account: true,
            provisioned: false,
        },
        Row {
            case: "a trust document denying the metadata principal",
            trust: Some(trust(
                "Deny",
                &format!("\"{METADATA_SERVICE_PRINCIPAL}\""),
                ASSUME_ROLE,
            )),
            this_account: true,
            provisioned: false,
        },
        Row {
            case: "a role this account's IAM holds no record of",
            trust: None,
            this_account: true,
            provisioned: false,
        },
        Row {
            case: "a role of another account, which this emulator's IAM is not the one to ask",
            trust: None,
            this_account: false,
            provisioned: true,
        },
    ]
}

/// What the annotation table of `body` must report for `row`, at `when` in its life.
fn assert_annotation(row: &Row, body: &str, expected_status: &str) {
    let (status, code, message) = annotation_result(body);
    assert_eq!(
        status.as_deref(),
        Some(expected_status),
        "case {:?}: expected the annotation TableStatus {expected_status}, got {body:?}",
        row.case
    );
    let expected_code = (!row.provisioned).then_some(ANNOTATION_ROLE_PERMISSIONS_CODE);
    assert_eq!(
        code.as_deref(),
        expected_code,
        "case {:?}: expected the ErrorCode {expected_code:?}, got {code:?}",
        row.case
    );
    let expected_message = (!row.provisioned).then_some(ANNOTATION_ROLE_PERMISSIONS_MESSAGE);
    assert_eq!(
        message.as_deref(),
        expected_message,
        "case {:?}: expected the ErrorMessage {expected_message:?}, got {message:?}",
        row.case
    );
}

#[tokio::test]
async fn the_error_code_of_an_annotation_role_without_the_permissions_is_the_published_one() {
    // Spelled out by hand, so the wire value a client branches on is pinned here and not
    // read out of the code under test.
    assert_eq!(
        ANNOTATION_ROLE_PERMISSIONS_CODE, "InsufficientAnnotationRolePermissions",
        "the ErrorDetails code of an annotation role that does not grant the metadata \
         service what reading annotations needs"
    );
}

#[tokio::test]
async fn a_create_provisions_the_annotation_table_only_under_a_role_iam_shows_can_be_assumed() {
    for row in rows() {
        let emulator = Emulator::new();
        let bucket = "annotation-role-create";
        emulator.create_bucket(row.case, bucket).await;
        if let Some(trust) = row.trust.as_deref() {
            emulator.create_role(row.case, trust).await;
        }
        let account = match row.this_account {
            true => engine::emulator_account_id(),
            false => ANOTHER_ACCOUNT,
        };

        // The create is SERVED whatever the role: a table that cannot be provisioned is a
        // provisioning outcome the read reports, not a refusal of this call.
        let created = emulator
            .create_configuration(bucket, "ENABLED", Some(&role_arn(account)))
            .await;
        assert_eq!(
            created.status,
            200,
            "case {:?}: the create must answer the modelled 200, got {} with {:?}",
            row.case,
            created.status,
            text(&created)
        );

        // Straight after the create, a table that is being provisioned is still `CREATING`;
        // one that could not be provisioned reports `FAILED` from the create onwards.
        let body = emulator.read_configuration(bucket).await;
        let straight_after = match row.provisioned {
            true => CREATING,
            false => FAILED,
        };
        assert_annotation(&row, &body, straight_after);

        // Once the span the store recorded has passed, a provisioned table is in service and
        // a failed one is still failed: nothing is going to create it.
        wait_for_the_tables();
        let later = emulator.read_configuration(bucket).await;
        let in_the_end = match row.provisioned {
            true => ACTIVE,
            false => FAILED,
        };
        assert_annotation(&row, &later, in_the_end);
    }
}

#[tokio::test]
async fn an_update_that_enables_the_annotation_table_is_judged_by_the_same_role() {
    for row in rows() {
        let emulator = Emulator::new();
        let bucket = "annotation-role-update";
        emulator.create_bucket(row.case, bucket).await;
        if let Some(trust) = row.trust.as_deref() {
            emulator.create_role(row.case, trust).await;
        }
        let account = match row.this_account {
            true => engine::emulator_account_id(),
            false => ANOTHER_ACCOUNT,
        };

        // A configuration whose annotation table is DISABLED, so the UPDATE below is what
        // provisions one — and the journal table is left to reach service first, which is
        // what every update of a table of a configuration waits for.
        let created = emulator.create_configuration(bucket, "DISABLED", None).await;
        assert_eq!(
            created.status,
            200,
            "case {:?}: the starting configuration must be created, got {} with {:?}",
            row.case,
            created.status,
            text(&created)
        );
        wait_for_the_tables();

        let updated = emulator
            .enable_annotation_table(bucket, &role_arn(account))
            .await;
        assert_eq!(
            updated.status,
            200,
            "case {:?}: the update must answer the modelled 200, got {} with {:?}",
            row.case,
            updated.status,
            text(&updated)
        );

        let body = emulator.read_configuration(bucket).await;
        let straight_after = match row.provisioned {
            true => CREATING,
            false => FAILED,
        };
        assert_annotation(&row, &body, straight_after);

        wait_for_the_tables();
        let later = emulator.read_configuration(bucket).await;
        let in_the_end = match row.provisioned {
            true => ACTIVE,
            false => FAILED,
        };
        assert_annotation(&row, &later, in_the_end);
    }
}
