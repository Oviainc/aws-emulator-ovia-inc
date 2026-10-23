// SPDX-License-Identifier: CC0-1.0

//! What MGN's REST front end does with a request body, end to end over HTTP.
//!
//! Every MGN operation sits behind a front end that validates the request body against
//! the model BEFORE the operation runs. The rule this suite pins:
//!
//! * a member whose VALUE violates a constraint the model declares on it — `@length`,
//!   `@pattern`, `@range`, an enum's value set — is refused with HTTP 400 and the error
//!   code `BadRequestException`, at any depth of the body and in any operation, and the
//!   call reaches no handler and stores nothing;
//! * a body that satisfies every modelled constraint is NOT refused that way: it reaches
//!   the operation, which answers with its own modelled code (or succeeds);
//! * the refusals an OPERATION makes itself keep their modelled codes — the modelled
//!   `ValidationException` for a rule a handler checks, `UninitializedAccountException`
//!   for account state — and so does the framework's refusal of an absent `@required`
//!   member. That separation is the point: a client branches on the code to tell a
//!   malformed body from a service-state refusal.
//!
//! The recorded cases (a malformed `CreateConnector.ssmInstanceID` and a malformed
//! `CreateNetworkMigrationDefinition.targetNetwork.inboundCidr`) are two rows of the
//! table below; every other row varies the same rule along a different axis — which
//! constraint fails, which member, how deep in the body, and which operation.
//!
//! The harness (`Mgn`, `error_code`, the account constant) is the one
//! `application_lifecycle.rs` defines; `call` takes the operation's modelled URI path.

use serde_json::{json, Value};

use crate::application_lifecycle::{error_code, Mgn, EMULATOR_ACCOUNT};

/// The code MGN's front end refuses a body that fails model validation with. It is not a
/// shape in this service's model: it is the front end's own code, which is what makes it
/// discriminating against the operations' modelled 400s.
const BAD_REQUEST: &str = "BadRequestException";

/// The modelled client-input error (`@httpError 400`) an mgn operation raises itself.
const VALIDATION: &str = "ValidationException";

/// The modelled account-state error (`@httpError 400`) an mgn operation raises for a
/// caller whose account has never been initialized.
const UNINITIALIZED: &str = "UninitializedAccountException";

// The operations' modelled `http.uri`s, without the leading slash the harness adds
// (`smithy_query(query="operation",operation="CreateConnector,CreateApplication,ListApplications,CreateNetworkMigrationDefinition")`).
const CREATE_CONNECTOR: &str = "CreateConnector";
const CREATE_APPLICATION: &str = "CreateApplication";
const LIST_APPLICATIONS: &str = "ListApplications";
const CREATE_DEFINITION: &str = "network-migration/CreateNetworkMigrationDefinition";

/// A well-formed `SsmInstanceID` at the modelled minimum length: `i-` + 17 = 19 (the
/// model's `@length` 19..20 and `@pattern (^i-[0-9a-zA-Z]{17}$)|(^mi-[0-9a-zA-Z]{17}$)`).
const SSM_INSTANCE_MIN_LEN: &str = "i-0123456789abcdefg";

/// A well-formed `SsmInstanceID` at the modelled maximum length: `mi-` + 17 = 20.
const SSM_INSTANCE_MAX_LEN: &str = "mi-0123456789abcdefg";

/// A `targetS3Configuration` as `CreateNetworkMigrationDefinition` requires it.
fn target_s3() -> Value {
    json!({"s3Bucket": "migration-artifacts", "s3BucketOwner": EMULATOR_ACCOUNT})
}

/// A `CreateNetworkMigrationDefinition` body with `targetNetwork` replaced by `network`,
/// so a row varies only that nested structure.
fn definition_with(network: Value) -> Value {
    json!({
        "name": "network-plan",
        "targetS3Configuration": target_s3(),
        "targetNetwork": network,
    })
}

/// A `SourceConfiguration` as the model requires it (`sourceEnvironment` and
/// `sourceS3Configuration`, whose three members are all required).
fn source_configuration() -> Value {
    json!({
        "sourceEnvironment": "NSX",
        "sourceS3Configuration": {
            "s3Bucket": "nsx-exports",
            "s3BucketOwner": EMULATOR_ACCOUNT,
            "s3Key": "exports/nsx.json",
        },
    })
}

/// Bodies whose member value violates a constraint the model declares, one row per axis
/// the rule generalises over: which constraint (`@length` min and max, `@pattern`,
/// `@range` min and max, an enum's value set), which member, how deep (top-level member,
/// a nested structure's member, a map entry's value, an element of a list), and which
/// operation.
fn malformed_bodies() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        (
            // The recorded case: 27 characters, past SsmInstanceID's @length maximum of 20.
            "ssmInstanceID far longer than the modelled maximum",
            CREATE_CONNECTOR,
            json!({"name": "migration-connector", "ssmInstanceID": "not-a-valid-ssm-instance-id"}),
        ),
        (
            // 18 characters: one short of SsmInstanceID's @length minimum of 19.
            "ssmInstanceID one character short of the modelled minimum",
            CREATE_CONNECTOR,
            json!({"name": "migration-connector", "ssmInstanceID": "i-0123456789abcdef"}),
        ),
        (
            // 19 characters, so inside @length, but `x-` is neither `i-` nor `mi-`.
            "ssmInstanceID at a modelled length but outside the pattern",
            CREATE_CONNECTOR,
            json!({"name": "migration-connector", "ssmInstanceID": "x-0123456789abcdefg"}),
        ),
        (
            // ConnectorName's @pattern is ^[A-Za-z0-9_-]+$, which no space satisfies.
            "name outside ConnectorName's pattern",
            CREATE_CONNECTOR,
            json!({"name": "migration connector", "ssmInstanceID": SSM_INSTANCE_MIN_LEN}),
        ),
        (
            // ConnectorName's @length minimum is 1.
            "empty name, below ConnectorName's length minimum",
            CREATE_CONNECTOR,
            json!({"name": "", "ssmInstanceID": SSM_INSTANCE_MIN_LEN}),
        ),
        (
            // One level down: ConnectorSsmCommandConfig.outputS3BucketName targets
            // S3BucketName, @pattern ^[a-zA-Z0-9.\-_]{1,255}$.
            "nested ssmCommandConfig.outputS3BucketName outside its pattern",
            CREATE_CONNECTOR,
            json!({
                "name": "migration-connector",
                "ssmInstanceID": SSM_INSTANCE_MIN_LEN,
                "ssmCommandConfig": {
                    "s3OutputEnabled": true,
                    "cloudWatchOutputEnabled": false,
                    "outputS3BucketName": "not a bucket name",
                },
            }),
        ),
        (
            // ApplicationName's @pattern is ^[^\s\x00]( *[^\s\x00])*$: no leading space.
            "name with a leading space, outside ApplicationName's pattern",
            CREATE_APPLICATION,
            json!({"name": " billing-app"}),
        ),
        (
            // AccountID is @length 12..12.
            "accountID shorter than the modelled 12..12",
            CREATE_APPLICATION,
            json!({"name": "billing-app", "accountID": "12345"}),
        ),
        (
            // A map ENTRY's value: TagValue is @length 0..256.
            "a tag value longer than the modelled 256",
            CREATE_APPLICATION,
            json!({"name": "billing-app", "tags": {"env": "x".repeat(257)}}),
        ),
        (
            // MaxResultsType is @range 1..1000.
            "maxResults below the modelled range minimum",
            LIST_APPLICATIONS,
            json!({"maxResults": 0}),
        ),
        (
            "maxResults above the modelled range maximum",
            LIST_APPLICATIONS,
            json!({"maxResults": 1001}),
        ),
        (
            // The recorded case: Cidr's @pattern over a dotted quad with /16../28.
            "nested targetNetwork.inboundCidr that is not a CIDR at all",
            CREATE_DEFINITION,
            definition_with(json!({"topology": "ISOLATED_VPC", "inboundCidr": "not-a-cidr"})),
        ),
        (
            "a prefix length of /15, one below the modelled 16",
            CREATE_DEFINITION,
            definition_with(json!({"topology": "ISOLATED_VPC", "inboundCidr": "10.0.0.0/15"})),
        ),
        (
            "a prefix length of /29, one above the modelled 28",
            CREATE_DEFINITION,
            definition_with(json!({"topology": "ISOLATED_VPC", "inboundCidr": "10.0.0.0/29"})),
        ),
        (
            // The pattern's first three octets stop at 254 (25[0-4]|…), only the last
            // reaches 255.
            "a leading octet of 255, which the modelled pattern stops at 254",
            CREATE_DEFINITION,
            definition_with(json!({"topology": "ISOLATED_VPC", "inboundCidr": "255.0.0.1/16"})),
        ),
        (
            // Another member of the same nested structure, the others well formed.
            "outboundCidr malformed while inboundCidr is well formed",
            CREATE_DEFINITION,
            definition_with(json!({
                "topology": "HUB_AND_SPOKE",
                "inboundCidr": "10.0.0.0/16",
                "outboundCidr": "10.0.0.0/8",
                "inspectionCidr": "10.0.2.0/24",
            })),
        ),
        (
            // Two levels down and through a LIST: cidrMappings[].originalCidr targets
            // Cidr too. `USE_EXISTING` is the strategy the handler's own cidrMappings
            // rule allows, so the only defect in this body is the malformed value.
            "a cidrMappings element's originalCidr malformed",
            CREATE_DEFINITION,
            json!({
                "name": "network-plan",
                "targetS3Configuration": target_s3(),
                "targetNetwork": {"topology": "ISOLATED_VPC"},
                "vpcProvisioningStrategy": "USE_EXISTING",
                "cidrMappings": [{"originalCidr": "10.0.0.0/16", "updatedCidr": "10.0.0.0/33"}],
            }),
        ),
        (
            // A LIST's own @length: SourceConfigurationList is 0..2.
            "sourceConfigurations with three elements, above the modelled list maximum of 2",
            CREATE_DEFINITION,
            json!({
                "name": "network-plan",
                "targetS3Configuration": target_s3(),
                "targetNetwork": {"topology": "ISOLATED_VPC"},
                "sourceConfigurations": [
                    source_configuration(),
                    source_configuration(),
                    source_configuration(),
                ],
            }),
        ),
        (
            // An enum's value set: TargetNetworkTopology is ISOLATED_VPC | HUB_AND_SPOKE.
            "a topology outside TargetNetworkTopology's value set",
            CREATE_DEFINITION,
            definition_with(json!({"topology": "ISOLATED"})),
        ),
    ]
}

/// Bodies that satisfy every modelled constraint, including the published bounds of the
/// ones the table above steps past. None of these is the front end's refusal: each one
/// reaches its operation.
fn well_formed_bodies() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        (
            "ssmInstanceID at the modelled minimum length",
            CREATE_CONNECTOR,
            json!({"name": "migration-connector", "ssmInstanceID": SSM_INSTANCE_MIN_LEN}),
        ),
        (
            "ssmInstanceID in the mi- form, at the modelled maximum length",
            CREATE_CONNECTOR,
            json!({"name": "migration-connector", "ssmInstanceID": SSM_INSTANCE_MAX_LEN}),
        ),
        (
            "a single-character connector name, at ConnectorName's length minimum",
            CREATE_CONNECTOR,
            json!({"name": "c", "ssmInstanceID": SSM_INSTANCE_MIN_LEN}),
        ),
        (
            "a tag value of exactly the modelled 256 characters",
            CREATE_APPLICATION,
            json!({"name": "billing-app", "tags": {"env": "x".repeat(256)}}),
        ),
        (
            "maxResults at the modelled range minimum",
            LIST_APPLICATIONS,
            json!({"maxResults": 1}),
        ),
        (
            "maxResults at the modelled range maximum",
            LIST_APPLICATIONS,
            json!({"maxResults": 1000}),
        ),
        (
            "a prefix length of /16, the modelled lower bound",
            CREATE_DEFINITION,
            definition_with(json!({"topology": "ISOLATED_VPC", "inboundCidr": "10.0.0.0/16"})),
        ),
        (
            "a prefix length of /28, the modelled upper bound",
            CREATE_DEFINITION,
            definition_with(json!({"topology": "ISOLATED_VPC", "inboundCidr": "10.0.0.0/28"})),
        ),
        (
            // The pattern's LAST octet does reach 255, and its first three reach 254.
            "a trailing octet of 255 and a leading octet of 254",
            CREATE_DEFINITION,
            definition_with(json!({"topology": "HUB_AND_SPOKE", "inboundCidr": "254.0.0.255/16"})),
        ),
        (
            "both modelled topologies with no optional CIDRs at all",
            CREATE_DEFINITION,
            definition_with(json!({"topology": "HUB_AND_SPOKE"})),
        ),
    ]
}

/// A request body whose member value violates a modelled constraint is refused with HTTP
/// 400 `BadRequestException`, carried in the body's `__type` and in the
/// `x-amzn-errortype` header restJson1 resolves an error shape from.
#[tokio::test]
async fn a_body_violating_a_modelled_member_constraint_is_a_bad_request() {
    let mgn = Mgn::new();
    mgn.initialize().await;

    for (case, operation, body) in malformed_bodies() {
        let (status, error_type, answer) = mgn.call_reporting_error_type(operation, &body).await;
        assert_eq!(
            (status, error_code(&answer)),
            (400, BAD_REQUEST),
            "case {case:?} on {operation}: expected 400 {BAD_REQUEST}, got {status} with {answer:?}"
        );
        assert_eq!(
            error_type.as_deref(),
            Some(BAD_REQUEST),
            "case {case:?} on {operation}: expected x-amzn-errortype {BAD_REQUEST}, got {error_type:?}"
        );
    }
}

/// A body that satisfies every modelled constraint — including at each published bound —
/// is never the front end's refusal: it reaches the operation, which answers for itself.
#[tokio::test]
async fn a_well_formed_body_is_not_refused_by_the_front_end() {
    let mgn = Mgn::new();
    mgn.initialize().await;

    for (case, operation, body) in well_formed_bodies() {
        let (status, answer) = mgn.call(operation, &body).await;
        assert_ne!(
            error_code(&answer),
            BAD_REQUEST,
            "case {case:?} on {operation}: a well-formed body must reach the operation, got \
             {status} with {answer:?}"
        );
    }
}

/// The front end's refusal reports no member path and no submitted value: the message is
/// the front end's own sentence, not a rendering of the violation, so a caller's value is
/// not echoed back to it.
#[tokio::test]
async fn the_front_end_refusal_echoes_nothing_from_the_request() {
    let mgn = Mgn::new();
    mgn.initialize().await;

    let echoed = [
        (
            "ssmInstanceID",
            CREATE_CONNECTOR,
            "not-a-valid-ssm-instance-id",
            json!({"name": "migration-connector", "ssmInstanceID": "not-a-valid-ssm-instance-id"}),
        ),
        (
            "inboundCidr",
            CREATE_DEFINITION,
            "not-a-cidr",
            definition_with(json!({"topology": "ISOLATED_VPC", "inboundCidr": "not-a-cidr"})),
        ),
    ];

    for (member, operation, value, body) in echoed {
        let (status, answer) = mgn.call(operation, &body).await;
        assert_eq!(
            (status, error_code(&answer)),
            (400, BAD_REQUEST),
            "case {member:?}: expected 400 {BAD_REQUEST}, got {status} with {answer:?}"
        );
        let message = answer["message"].as_str().unwrap_or_default();
        assert!(
            !message.is_empty(),
            "case {member:?}: the refusal must carry a message, got {answer:?}"
        );
        assert!(
            !message.contains(value) && !message.contains(member),
            "case {member:?}: the front end states no member or submitted value, got \
             {message:?}"
        );
    }
}

/// An absent `@required` member is a different refusal, and keeps the modelled
/// `ValidationException`: the front-end code is for a VALUE that failed a constraint.
#[tokio::test]
async fn an_absent_required_member_keeps_the_modelled_validation_exception() {
    let mgn = Mgn::new();
    mgn.initialize().await;

    let absent = [
        (
            "CreateConnector without ssmInstanceID",
            CREATE_CONNECTOR,
            json!({"name": "migration-connector"}),
        ),
        (
            "CreateConnector without name",
            CREATE_CONNECTOR,
            json!({"ssmInstanceID": SSM_INSTANCE_MIN_LEN}),
        ),
        (
            "CreateApplication without name",
            CREATE_APPLICATION,
            json!({}),
        ),
        (
            "CreateNetworkMigrationDefinition without targetNetwork",
            CREATE_DEFINITION,
            json!({"name": "network-plan", "targetS3Configuration": target_s3()}),
        ),
        (
            "CreateNetworkMigrationDefinition without the nested targetNetwork.topology",
            CREATE_DEFINITION,
            definition_with(json!({"inboundCidr": "10.0.0.0/16"})),
        ),
    ];

    for (case, operation, body) in absent {
        let (status, error_type, answer) = mgn.call_reporting_error_type(operation, &body).await;
        assert_eq!(
            (status, error_code(&answer)),
            (400, VALIDATION),
            "case {case:?}: expected 400 {VALIDATION}, got {status} with {answer:?}"
        );
        assert_eq!(
            error_type.as_deref(),
            Some(VALIDATION),
            "case {case:?}: expected x-amzn-errortype {VALIDATION}, got {error_type:?}"
        );
    }
}

/// A refusal an OPERATION makes keeps its own modelled code, so the front end's code
/// stays discriminating: a rule a handler checks is the modelled `ValidationException`,
/// and an uninitialized account is `UninitializedAccountException`.
#[tokio::test]
async fn a_refusal_the_operation_makes_keeps_its_modelled_code() {
    let mgn = Mgn::new();

    // The account gate, reached only because the body is well formed: the issue's own
    // discriminator between a malformed field and a service-state refusal.
    let (status, answer) = mgn
        .call(CREATE_APPLICATION, &json!({"name": "billing-app"}))
        .await;
    assert_eq!(
        (status, error_code(&answer)),
        (400, UNINITIALIZED),
        "a well-formed body on an uninitialized account must be {UNINITIALIZED}, got {status} \
         with {answer:?}"
    );

    mgn.initialize().await;

    // A pagination token the service never issued: well formed against the model
    // (PaginationToken is @length 0..2048 with no pattern), refused by the handler.
    let (status, answer) = mgn
        .call(LIST_APPLICATIONS, &json!({"nextToken": "not-a-token"}))
        .await;
    assert_eq!(
        (status, error_code(&answer)),
        (400, VALIDATION),
        "an unissued nextToken must stay the modelled {VALIDATION}, got {status} with {answer:?}"
    );

    // A cross-member rule the handler owns: well-formed CIDR mappings are allowed only
    // with `USE_EXISTING`.
    let (status, answer) = mgn
        .call(
            CREATE_DEFINITION,
            &json!({
                "name": "network-plan",
                "targetS3Configuration": target_s3(),
                "targetNetwork": {"topology": "ISOLATED_VPC"},
                "vpcProvisioningStrategy": "CREATE_NEW",
                "cidrMappings": [{"originalCidr": "10.0.0.0/16", "updatedCidr": "10.1.0.0/16"}],
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&answer)),
        (400, VALIDATION),
        "cidrMappings outside USE_EXISTING must stay the modelled {VALIDATION}, got {status} \
         with {answer:?}"
    );
}

/// A refused body changes nothing: the front end answers before any handler runs, so no
/// definition is stored and the operation's own quota is untouched.
#[tokio::test]
async fn a_refused_body_stores_nothing() {
    let mgn = Mgn::new();
    mgn.initialize().await;

    for (case, operation, body) in malformed_bodies() {
        let (status, answer) = mgn.call(operation, &body).await;
        assert_eq!(
            (status, error_code(&answer)),
            (400, BAD_REQUEST),
            "case {case:?} on {operation}: expected 400 {BAD_REQUEST}, got {status} with \
             {answer:?}"
        );
    }

    let store = mgn.store();
    let state = store.lock();
    assert!(
        state
            .network_migration_definitions_of(EMULATOR_ACCOUNT)
            .is_empty(),
        "a refused CreateNetworkMigrationDefinition must store no definition, got {:?}",
        state
            .network_migration_definitions_of(EMULATOR_ACCOUNT)
            .len()
    );
    assert!(
        state.applications_of(EMULATOR_ACCOUNT).is_empty(),
        "a refused CreateApplication must store no application, got {:?}",
        state.applications_of(EMULATOR_ACCOUNT).len()
    );
}
