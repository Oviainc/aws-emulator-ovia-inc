// SPDX-License-Identifier: CC0-1.0

//! The AUDIT leg of this service: every call it serves is a management event of
//! `iam.amazonaws.com`, recorded in the account's event history where CloudTrail's
//! `LookupEvents` reports it.
//!
//! RULE: a service that serves a control-plane API call records that call in the account's
//! management-event log — the operation's own name as `eventName`, this service's
//! `eventSource`, the Region its event history lives in (one Region, because this service
//! is global) and the caller the call was served for, the parameters the request carried,
//! the values a MUTATING call answered with as `responseElements` (a reporting call has
//! none, and `readOnly` is what tells the two apart), and, for a failed call, the FAULT
//! SHAPE's name as `errorCode` with the message the caller was answered with. A call this
//! emulator has not implemented answers 501 and is recorded as nothing: it was never
//! served.
//!
//! RULE, for the credential material this service's calls carry: a member whose value its
//! records withhold is left OUT of the recorded document entirely — no key for it at all,
//! not a key carrying a placeholder — so neither a secret the service minted nor a
//! password a caller sent appears anywhere in a record.
//!
//! The tables below span what those rules generalise over rather than the one call a
//! recording made: the kinds of call (create, read, listing, tag write, delete), both
//! outcomes (served and refused), an operation that is still a stub, the Region a caller
//! signed for, and every withheld member reachable end to end — on its request side and
//! on its response side.
//!
//! Every call goes over the wire through the same `MultiServiceRouter` the production
//! router uses, as a real awsQuery form POST; the records are read back out of the SHARED
//! log in the engine the service was built from — the one store CloudTrail's `LookupEvents`
//! reads.

use serde_json::Value;
use service_common::management_events::{ManagementEvent, ManagementEventLog};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::service::management_events::{
    EVENT_REGION, EVENT_SOURCE,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use crate::saml_providers::{metadata_document, MetadataForm};
use crate::server_certificates::material;
use crate::user_lifecycle::{call_signed_by, call_signed_in_region, element, router_with_engine};

/// The access key id every call below is signed with unless a row names another: a
/// long-term key, which is what makes the recorded caller this emulator's own IAM user.
const ACCESS_KEY_ID: &str = "AKIDAUDITSUITE";

/// The literal CloudTrail stores in place of a REDACTED member's value. Nothing this
/// service records carries it: what it withholds, it withholds by omission, so the literal
/// is here to be asserted ABSENT.
const REDACTED: &str = "HIDDEN_DUE_TO_SECURITY_REASONS";

/// The emulator one case drives: the router the calls go through, and the engine whose
/// shared management-event log they are recorded in.
struct Audited {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Audited {
    /// A router carrying only this service over one shared engine.
    fn new() -> Self {
        let (router, engine) = router_with_engine();
        Self { router, engine }
    }

    /// One awsQuery call signed with [`ACCESS_KEY_ID`], as the HTTP status and the XML body
    /// it answered.
    async fn call(&self, action: &str, params: &[(&str, &str)]) -> (u16, String) {
        call_signed_by(&self.router, Some(ACCESS_KEY_ID), action, params).await
    }

    /// One awsQuery call whose SigV4 credential scope names `region`.
    async fn call_in(&self, region: &str, action: &str, params: &[(&str, &str)]) -> (u16, String) {
        call_signed_in_region(&self.router, Some(ACCESS_KEY_ID), region, action, params).await
    }

    /// One awsQuery call presented as signed by `akid`, for the operations that resolve the
    /// caller's identity from the signing key.
    async fn call_as(
        &self,
        akid: &str,
        action: &str,
        params: &[(&str, &str)],
    ) -> (u16, String) {
        call_signed_by(&self.router, Some(akid), action, params).await
    }

    /// Every record in the shared log, oldest first, as the CloudTrail event document a
    /// lookup reports.
    fn records(&self) -> Vec<Value> {
        self.with_log(ManagementEvent::to_cloudtrail_event_json)
    }

    /// The records of the calls to `event_name`, oldest first.
    fn records_of(&self, event_name: &str) -> Vec<Value> {
        self.records()
            .into_iter()
            .filter(|event| event.get("eventName") == Some(&Value::String(event_name.to_string())))
            .collect()
    }

    /// The most recent record of a call to `event_name`.
    fn record_of(&self, case: &str, event_name: &str) -> Value {
        self.records_of(event_name)
            .pop()
            .unwrap_or_else(|| panic!("case {case:?}: expected a {event_name} record, got none"))
    }

    /// Every record as one block of JSON text, for asserting that a secret is in none of
    /// them.
    fn whole_log(&self) -> String {
        self.records()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<String>>()
            .join("\n")
    }

    /// `read` applied to every record in the shared log, oldest first.
    fn with_log<T>(&self, read: impl Fn(&ManagementEvent) -> T) -> Vec<T> {
        let log = self.engine.state::<ManagementEventLog>();
        let held = log.lock();
        held.iter().map(read).collect()
    }
}

/// A string member of a record, or `None` when the record does not carry it.
fn member(event: &Value, pointer: &str) -> Option<String> {
    event
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// The `<Code>` of an awsQuery error body, or an empty string for a body with none.
fn wire_code(body: &str) -> String {
    let open = "<Code>";
    let close = "</Code>";
    match (body.find(open), body.find(close)) {
        (Some(start), Some(end)) if start + open.len() <= end => {
            body[start + open.len()..end].to_string()
        }
        _ => String::new(),
    }
}

/// The query parameters of one call, owned so a table row can hold them.
type Params = Vec<(&'static str, String)>;

/// `[(name, value)]` as the call helper takes it.
fn borrowed<'a>(params: &'a [(&'static str, String)]) -> Vec<(&'static str, &'a str)> {
    params
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect()
}

/// One row of the served-call table: the case, the action, its parameters, the HTTP status
/// the caller sees, the `<Code>` on the wire and the `errorCode` the record must carry.
type ServedRow = (
    &'static str,
    &'static str,
    Params,
    u16,
    Option<&'static str>,
    Option<&'static str>,
);

/// RULE: every call this service SERVES is recorded, whatever it answered — and one it has
/// not implemented is recorded as nothing.
///
/// The rows differ along the kind of call (create, read, listing, tag write, delete), along
/// the outcome (served, refused) and along whether the operation is implemented at all,
/// which is what the rule generalises over. A refused row also states the relation the
/// recorded `errorCode` has to the code the caller saw: the record names the fault SHAPE,
/// the wire carries that shape's `awsQueryError` code.
#[tokio::test]
async fn every_served_call_is_recorded_and_an_unimplemented_one_is_not() {
    let audited = Audited::new();
    let (status, body) = audited.call("CreateUser", &[("UserName", "audit-user")]).await;
    assert_eq!(
        status, 200,
        "setup: the create must be served, got {status} with {body}"
    );

    let rows: Vec<ServedRow> = vec![
        (
            "a read",
            "GetUser",
            vec![("UserName", "audit-user".to_string())],
            200,
            None,
            None,
        ),
        (
            "a listing",
            "ListUsers",
            Vec::new(),
            200,
            None,
            None,
        ),
        (
            "a tag write",
            "TagUser",
            vec![
                ("UserName", "audit-user".to_string()),
                ("Tags.member.1.Key", "owner".to_string()),
                ("Tags.member.1.Value", "audit".to_string()),
            ],
            200,
            None,
            None,
        ),
        (
            "a refused read",
            "GetUser",
            vec![("UserName", "no-such-user".to_string())],
            404,
            Some("NoSuchEntity"),
            Some("NoSuchEntityException"),
        ),
        (
            "a refused create",
            "CreateUser",
            vec![("UserName", "audit-user".to_string())],
            409,
            Some("EntityAlreadyExists"),
            Some("EntityAlreadyExistsException"),
        ),
        (
            "a delete",
            "DeleteUser",
            vec![("UserName", "audit-user".to_string())],
            200,
            None,
            None,
        ),
        (
            // Still a stub in this emulator: it answers 501, so nothing was served and
            // nothing is recorded.
            "an unimplemented operation",
            "ListMFADevices",
            Vec::new(),
            501,
            None,
            None,
        ),
    ];

    for (case, action, params, expected_status, expected_wire, expected_error) in &rows {
        let before = audited.records().len();
        let (status, body) = audited.call(action, &borrowed(params)).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {body}"
        );
        let records = audited.records();
        if *expected_status == 501 {
            assert_eq!(
                records.len(),
                before,
                "case {case:?}: an unimplemented call records nothing, got {records:?}"
            );
            continue;
        }
        assert_eq!(
            records.len(),
            before + 1,
            "case {case:?}: expected exactly one new record, got {records:?}"
        );
        let event = records.last().expect("checked just above");
        assert_eq!(
            member(event, "/eventName").as_deref(),
            Some(*action),
            "case {case:?}: the record names the operation, got {event}"
        );
        assert_eq!(
            member(event, "/eventSource").as_deref(),
            Some(EVENT_SOURCE),
            "case {case:?}: the record names this service as its source, got {event}"
        );
        assert_eq!(
            member(event, "/awsRegion").as_deref(),
            Some(EVENT_REGION),
            "case {case:?}: the record is filed in this service's one Region, got {event}"
        );
        assert_eq!(
            member(event, "/userIdentity/accessKeyId").as_deref(),
            Some(ACCESS_KEY_ID),
            "case {case:?}: the record names the credential the call was signed with, got \
             {event}"
        );
        assert_eq!(
            member(event, "/eventType").as_deref(),
            Some("AwsApiCall"),
            "case {case:?}: a recorded API call is an AwsApiCall, got {event}"
        );
        assert_eq!(
            member(event, "/errorCode").as_deref(),
            *expected_error,
            "case {case:?}: expected errorCode {expected_error:?}, got {event}"
        );
        if let Some(expected_wire) = expected_wire {
            let answered = wire_code(&body);
            assert_eq!(
                answered, *expected_wire,
                "case {case:?}: expected the wire code {expected_wire:?}, got {answered:?} in \
                 {body}"
            );
            assert!(
                member(event, "/errorMessage").is_some_and(|message| !message.is_empty()),
                "case {case:?}: the record carries the message the caller was answered with, \
                 got {event}"
            );
            assert_eq!(
                event.get("responseElements"),
                Some(&Value::Null),
                "case {case:?}: a failed call answered nothing, got {event}"
            );
        }
    }
}

/// RULE: `readOnly` says whether the call only REPORTED state, and a reporting call records
/// no `responseElements` at all while a mutating one records what it answered.
///
/// The rows span a create, a read of one entity, a listing and a tag write — the kinds of
/// call whose own semantics decide the flag.
#[tokio::test]
async fn a_read_and_a_write_are_told_apart_by_what_the_record_carries() {
    let audited = Audited::new();
    for (case, action, params) in [
        ("the create", "CreateUser", vec![("UserName", "audit-kinds")]),
        ("the read", "GetUser", vec![("UserName", "audit-kinds")]),
        ("the listing", "ListUsers", Vec::new()),
        (
            "the tag write",
            "TagUser",
            vec![
                ("UserName", "audit-kinds"),
                ("Tags.member.1.Key", "team"),
                ("Tags.member.1.Value", "audit"),
            ],
        ),
    ] {
        let (status, body) = audited.call(action, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the call must be served, got {status} with {body}"
        );
    }

    // (case, the operation, readOnly, whether the record carries responseElements)
    for (case, event_name, read_only, answers) in [
        ("the create", "CreateUser", false, true),
        ("the read", "GetUser", true, false),
        ("the listing", "ListUsers", true, false),
        ("the tag write", "TagUser", false, true),
    ] {
        let event = audited.record_of(case, event_name);
        assert_eq!(
            event.get("readOnly"),
            Some(&Value::Bool(read_only)),
            "case {case:?}: expected readOnly {read_only}, got {event}"
        );
        let elements = event.get("responseElements").unwrap_or(&Value::Null);
        assert_eq!(
            !elements.is_null(),
            answers,
            "case {case:?}: expected responseElements present={answers}, got {event}"
        );
    }

    // The create's record carries what the call answered with, and the read's carries the
    // member the request named — both spelled as an event document spells a member.
    let created = audited.record_of("the create", "CreateUser");
    assert_eq!(
        member(&created, "/responseElements/user/userName").as_deref(),
        Some("audit-kinds"),
        "the create's record carries what it answered, got {created}"
    );
    let read = audited.record_of("the read", "GetUser");
    assert_eq!(
        member(&read, "/requestParameters/userName").as_deref(),
        Some("audit-kinds"),
        "the read's record carries the member the request named, got {read}"
    );
}

/// RULE: this service is GLOBAL, so its records are filed in the one Region its event
/// history lives in whatever Region a caller's credential scope named.
///
/// The rows differ along the Region the request was signed for, which is the only place a
/// request carries one.
#[tokio::test]
async fn every_record_is_filed_in_this_service_s_one_region() {
    let audited = Audited::new();
    for (case, region, user) in [
        ("the leader Region itself", EVENT_REGION, "audit-region-one"),
        ("another Region in the partition", "eu-west-1", "audit-region-two"),
        ("a Region on the other side", "ap-southeast-2", "audit-region-three"),
    ] {
        let (status, body) = audited
            .call_in(region, "CreateUser", &[("UserName", user)])
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the call must be served, got {status} with {body}"
        );
        let event = audited.record_of(case, "CreateUser");
        assert_eq!(
            member(&event, "/awsRegion").as_deref(),
            Some(EVENT_REGION),
            "case {case:?}: a call signed for {region} is still recorded in {EVENT_REGION}, \
             got {event}"
        );
    }
}

/// One row of the withholding table: the case, the JSON pointer at the member that must be
/// ABSENT from the record, the sibling pointers that must be present, and the secret that
/// must appear in no record at all.
struct Withheld {
    case: &'static str,
    event_name: &'static str,
    absent: Vec<&'static str>,
    present: Vec<&'static str>,
    secret: String,
}

/// RULE: a value held at a member this service's records withhold is not stored and the
/// member is not recorded AT ALL — the document carries no key for it, and the plaintext
/// appears nowhere in event history.
///
/// The rows are every withheld member (`service::management_events::OMITTED_MEMBERS`)
/// reachable end to end through an implemented operation, on the REQUEST side (a console
/// password on a create and on an update, both passwords of a change, an uploaded private
/// key, a SAML provider's private key) and on the RESPONSE side (the long-term secret a
/// key create answers with, the generated password of a service-specific credential and the
/// one its reset answers with) — so the rule is tested over the class rather than over the
/// one member a recording named.
#[tokio::test]
async fn a_withheld_member_is_omitted_from_a_record_entirely() {
    let audited = Audited::new();
    let mut rows: Vec<Withheld> = Vec::new();

    // A console password on the create that sets it, and on the administrative update that
    // replaces it. The user is created first: the record of that create is also what the
    // sibling assertions below read.
    let (status, body) = audited
        .call("CreateUser", &[("UserName", "audit-secrets")])
        .await;
    assert_eq!(
        status, 200,
        "setup: the user create must be served, got {status} with {body}"
    );
    let created_password = "Cr3ate-pl41nt3xt";
    let (status, body) = audited
        .call(
            "CreateLoginProfile",
            &[
                ("UserName", "audit-secrets"),
                ("Password", created_password),
                ("PasswordResetRequired", "true"),
            ],
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: the login-profile create must be served, got {status} with {body}"
    );
    rows.push(Withheld {
        case: "a login profile's password on the create",
        event_name: "CreateLoginProfile",
        absent: vec!["/requestParameters/password"],
        present: vec![
            "/requestParameters/userName",
            "/requestParameters/passwordResetRequired",
        ],
        secret: created_password.to_string(),
    });

    let updated_password = "Upd4te-pl41nt3xt";
    let (status, body) = audited
        .call(
            "UpdateLoginProfile",
            &[
                ("UserName", "audit-secrets"),
                ("Password", updated_password),
            ],
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: the login-profile update must be served, got {status} with {body}"
    );
    rows.push(Withheld {
        case: "the same password on the update",
        event_name: "UpdateLoginProfile",
        absent: vec!["/requestParameters/password"],
        present: vec!["/requestParameters/userName"],
        secret: updated_password.to_string(),
    });

    // The long-term secret a key create ANSWERS with: the withholding is on the response
    // side, and the secret is the one the credential store minted, read off the wire.
    let (status, minted) = audited
        .call("CreateAccessKey", &[("UserName", "audit-secrets")])
        .await;
    assert_eq!(
        status, 200,
        "setup: the access-key create must be served, got {status} with {minted}"
    );
    let minted_secret = element(&minted, "SecretAccessKey");
    let minted_key_id = element(&minted, "AccessKeyId");
    rows.push(Withheld {
        case: "the secret an access-key create answers with",
        event_name: "CreateAccessKey",
        absent: vec!["/responseElements/accessKey/secretAccessKey"],
        present: vec![
            "/responseElements/accessKey/userName",
            "/responseElements/accessKey/accessKeyId",
            "/responseElements/accessKey/status",
            "/responseElements/accessKey/createDate",
        ],
        secret: minted_secret,
    });

    // Both passwords a change carries, on a request the user itself signs. `ChangePassword`
    // is itself an IAM call the caller's policies decide, so the user is given that one
    // grant first — fixture, not what this row is about.
    let (status, body) = audited
        .call(
            "PutUserPolicy",
            &[
                ("UserName", "audit-secrets"),
                ("PolicyName", "allow-changepassword"),
                (
                    "PolicyDocument",
                    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"iam:ChangePassword","Resource":"*"}]}"#,
                ),
            ],
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: the fixture grant must be served, got {status} with {body}"
    );
    let changed_password = "Ch4nge-pl41nt3xt";
    let (status, body) = audited
        .call_as(
            &minted_key_id,
            "ChangePassword",
            &[
                ("OldPassword", updated_password),
                ("NewPassword", changed_password),
            ],
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: the password change must be served, got {status} with {body}"
    );
    rows.push(Withheld {
        case: "both passwords of a change",
        event_name: "ChangePassword",
        absent: vec![
            "/requestParameters/oldPassword",
            "/requestParameters/newPassword",
        ],
        present: Vec::new(),
        secret: changed_password.to_string(),
    });

    // The generated password of a service-specific credential, on the create and on the
    // reset that replaces it.
    let (status, issued) = audited
        .call(
            "CreateServiceSpecificCredential",
            &[
                ("UserName", "audit-secrets"),
                ("ServiceName", "codecommit.amazonaws.com"),
            ],
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: the credential create must be served, got {status} with {issued}"
    );
    let issued_password = element(&issued, "ServicePassword");
    let credential_id = element(&issued, "ServiceSpecificCredentialId");
    rows.push(Withheld {
        case: "the generated password of a service-specific credential",
        event_name: "CreateServiceSpecificCredential",
        absent: vec!["/responseElements/serviceSpecificCredential/servicePassword"],
        present: vec![
            "/responseElements/serviceSpecificCredential/serviceUserName",
            "/responseElements/serviceSpecificCredential/serviceSpecificCredentialId",
        ],
        secret: issued_password,
    });

    let (status, reset) = audited
        .call(
            "ResetServiceSpecificCredential",
            &[
                ("UserName", "audit-secrets"),
                ("ServiceSpecificCredentialId", &credential_id),
            ],
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: the credential reset must be served, got {status} with {reset}"
    );
    rows.push(Withheld {
        case: "the password a credential reset answers with",
        event_name: "ResetServiceSpecificCredential",
        absent: vec!["/responseElements/serviceSpecificCredential/servicePassword"],
        present: vec!["/responseElements/serviceSpecificCredential/serviceUserName"],
        secret: element(&reset, "ServicePassword"),
    });

    // The private key uploaded with a server certificate: a long multi-line PEM, so the row
    // also spans a withheld value that is not a short word.
    let certificate = material(&rcgen::PKCS_ECDSA_P256_SHA256, (2024, 2044), "audit.example");
    let (status, body) = audited
        .call(
            "UploadServerCertificate",
            &[
                ("ServerCertificateName", "audit-certificate"),
                ("CertificateBody", &certificate.certificate),
                ("PrivateKey", &certificate.private_key),
            ],
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: the certificate upload must be served, got {status} with {body}"
    );
    rows.push(Withheld {
        case: "a server certificate's private key",
        event_name: "UploadServerCertificate",
        absent: vec!["/requestParameters/privateKey"],
        present: vec![
            "/requestParameters/serverCertificateName",
            "/requestParameters/certificateBody",
        ],
        secret: certificate.private_key.clone(),
    });

    // The private key a SAML provider decrypts assertions with.
    let provider_key = format!(
        "-----BEGIN PRIVATE KEY----- {} -----END PRIVATE KEY-----",
        "QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVph".repeat(30)
    );
    let (status, body) = audited
        .call(
            "CreateSAMLProvider",
            &[
                ("Name", "audit-provider"),
                (
                    "SAMLMetadataDocument",
                    &metadata_document(MetadataForm::Entity, "https://idp.example.com", None),
                ),
                ("AddPrivateKey", &provider_key),
            ],
        )
        .await;
    assert_eq!(
        status, 200,
        "setup: the SAML provider create must be served, got {status} with {body}"
    );
    rows.push(Withheld {
        case: "a SAML provider's private key",
        event_name: "CreateSAMLProvider",
        absent: vec!["/requestParameters/addPrivateKey"],
        present: vec!["/requestParameters/name"],
        secret: provider_key,
    });

    let whole_log = audited.whole_log();
    for row in &rows {
        let case = row.case;
        let event = audited.record_of(case, row.event_name);
        for pointer in &row.absent {
            assert!(
                event.pointer(pointer).is_none(),
                "case {case:?}: the record must carry no {pointer} at all, got {event}"
            );
        }
        for pointer in &row.present {
            assert!(
                event.pointer(pointer).is_some(),
                "case {case:?}: the record must still carry {pointer}, got {event}"
            );
        }
        assert!(
            !whole_log.contains(&row.secret),
            "case {case:?}: the plaintext must appear in no record, got {whole_log}"
        );
    }
    assert!(
        !whole_log.contains(REDACTED),
        "no record of this service carries the redaction placeholder: what it withholds, it \
         omits, got {whole_log}"
    );
}

/// RULE: what a record carries IS what the call carried and answered — every member but
/// the withheld ones, named as an event document names a member and holding the value the
/// request sent or the response answered with.
///
/// The rows span the member forms a recorded document can hold: a plain string, a boolean,
/// a policy document carried as a string, a list of structures, and a member the request
/// omitted (which is recorded as absent rather than as null).
#[tokio::test]
async fn a_record_carries_what_the_call_carried_and_answered() {
    let audited = Audited::new();
    let trust = "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\
                 \"Principal\":{\"Service\":\"ec2.amazonaws.com\"},\"Action\":\
                 \"sts:AssumeRole\"}]}";

    for (case, action, params) in [
        (
            "a create with a path and no description",
            "CreateUser",
            vec![("UserName", "audit-carried"), ("Path", "/eng/tools/")],
        ),
        (
            "a create carrying a policy document and a description",
            "CreateRole",
            vec![
                ("RoleName", "audit-carried-role"),
                ("AssumeRolePolicyDocument", trust),
                ("Description", "the audited role"),
            ],
        ),
        (
            "a tag write carrying a list of structures",
            "TagUser",
            vec![
                ("UserName", "audit-carried"),
                ("Tags.member.1.Key", "owner"),
                ("Tags.member.1.Value", "audit"),
                ("Tags.member.2.Key", "env"),
                ("Tags.member.2.Value", "test"),
            ],
        ),
        (
            "a write carrying a boolean",
            "CreateLoginProfile",
            vec![
                ("UserName", "audit-carried"),
                ("Password", "S3cret-carried"),
                ("PasswordResetRequired", "true"),
            ],
        ),
    ] {
        let (status, body) = audited.call(action, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the call must be served, got {status} with {body}"
        );
    }

    // (case, the operation, the pointer into its record, the value that must be there)
    for (case, event_name, pointer, expected) in [
        (
            "the path the user create carried",
            "CreateUser",
            "/requestParameters/path",
            Value::String("/eng/tools/".to_string()),
        ),
        (
            "the user the create answered with",
            "CreateUser",
            "/responseElements/user/path",
            Value::String("/eng/tools/".to_string()),
        ),
        (
            "the policy document the role create carried",
            "CreateRole",
            "/requestParameters/assumeRolePolicyDocument",
            Value::String(trust.to_string()),
        ),
        (
            "the description the role create carried",
            "CreateRole",
            "/requestParameters/description",
            Value::String("the audited role".to_string()),
        ),
        (
            "the first tag of the list the tag write carried",
            "TagUser",
            "/requestParameters/tags/0/key",
            Value::String("owner".to_string()),
        ),
        (
            "the second tag of that same list",
            "TagUser",
            "/requestParameters/tags/1/value",
            Value::String("test".to_string()),
        ),
        (
            "the boolean the login-profile create carried",
            "CreateLoginProfile",
            "/requestParameters/passwordResetRequired",
            Value::Bool(true),
        ),
    ] {
        let event = audited.record_of(case, event_name);
        assert_eq!(
            event.pointer(pointer),
            Some(&expected),
            "case {case:?}: expected {pointer} to be {expected}, got {event}"
        );
    }

    // A member the request did not carry is ABSENT from the record rather than null: the
    // user create named no permissions boundary and the role create no path.
    for (case, event_name, pointer) in [
        (
            "a member the user create omitted",
            "CreateUser",
            "/requestParameters/permissionsBoundary",
        ),
        (
            "a member the role create omitted",
            "CreateRole",
            "/requestParameters/path",
        ),
    ] {
        let event = audited.record_of(case, event_name);
        assert!(
            event.pointer(pointer).is_none(),
            "case {case:?}: a member the request did not carry is recorded as absent, got \
             {event}"
        );
    }
}
