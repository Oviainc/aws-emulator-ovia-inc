// SPDX-License-Identifier: CC0-1.0

//! `LOAD DATA FROM S3` on a MySQL engine endpoint: the Amazon S3 import an Aurora MySQL DB
//! cluster and an RDS for MySQL DB instance serve.
//!
//! The rules these pin:
//!
//! * The statement PARSES and RUNS on the engines whose `LOAD DATA` grammar carries the
//!   `FROM S3` source: it reads the object the URI names out of Amazon S3 and loads its
//!   records as rows of the table, so a `SELECT` afterwards returns exactly the object's
//!   rows. The form of the object is the statement's own `FIELDS`/`LINES`/`IGNORE` clauses
//!   and its column list, and the objects are the `FILE`, `PREFIX` and `MANIFEST` sources.
//! * WHETHER the engine can read the object at all is the resource's own configuration, not
//!   the statement's: the engine acts under the IAM role one of the import-role parameters
//!   names AND that the resource has associated. Without such a role the statement still
//!   parses and fails where the engine would reach Amazon S3 — the server's S3 API error,
//!   loading nothing — so a client can tell a missing role association from a statement the
//!   engine does not have. The same statement on the same cluster succeeds the moment both
//!   halves are in place, which is what makes the refusal the configuration's and not the
//!   grammar's.
//! * An engine whose grammar has no `FROM S3` source answers its own parse error, because
//!   the statement is an Aurora MySQL and RDS for MySQL extension and not part of the wire.
//!
//! Amazon S3 and IAM are mounted beside this service on ONE engine, exactly as the assembled
//! router mounts them: the objects a load reads are objects that service holds, written
//! through its own `PutObject`, and the role the cluster is associated with is a role IAM
//! holds. The client is the MySQL protocol client the sibling endpoint suites drive, so what
//! is exercised is what a stock driver sends.

use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_parameter_groups::{create_group, modify};
use crate::db_clusters::the_cluster;
use crate::db_instance_engine_endpoint::{
    endpoint_of, mysql_login, parameter_group, MySqlSession, Refusal, Transport,
};
use crate::db_instances::{call, expect_ok, text, wait_for_settle, REGION};
use crate::rds_s3_integration::{create_bucket, emulator, put_object, role};
use service_com_amazonaws_rds_amazonrdsv19::state as st;

/// The master user each cluster and instance below is created with, and its password — test
/// credentials, and the only place in this suite they appear.
const MASTER_USER: &str = "emuadmin";
const MASTER_PASSWORD: &str = "emu-Secret-99";

/// The database the loaded tables live in.
const DATABASE: &str = "appdb";

/// The bucket the loaded objects live in.
const BUCKET: &str = "emu-rds-s3-import";

/// The server error number Aurora MySQL reports a failure of its own Amazon S3 call under,
/// which is what a client that has just configured an import branches on.
const ER_S3_API_ERROR: &str = "1871";

/// `ER_PARSE_ERROR`: the number a statement the engine's grammar does not define is refused
/// under, and the answer this suite asserts the import is NOT given.
const ER_PARSE_ERROR: &str = "1064";

/// `ER_NOT_SUPPORTED_YET`: the number a form of the statement this endpoint cannot read the
/// object by is refused under.
const ER_NOT_SUPPORTED_YET: &str = "1235";

/// `ER_BAD_DB_ERROR`: the number a reference to a database the resource has not got carries.
const ER_BAD_DB_ERROR: &str = "1049";

/// Create an Aurora MySQL cluster with `group` as its DB cluster parameter group, wait for it
/// to report `available`, and answer with the address it published.
async fn available_cluster(
    router: &MultiServiceRouter,
    identifier: &str,
    engine: &str,
    group: Option<&str>,
) -> (String, u16) {
    let mut request: Vec<(&str, String)> = vec![
        ("DBClusterIdentifier", identifier.to_string()),
        ("Engine", engine.to_string()),
        ("MasterUsername", MASTER_USER.to_string()),
        ("MasterUserPassword", MASTER_PASSWORD.to_string()),
        ("DatabaseName", DATABASE.to_string()),
    ];
    if let Some(group) = group {
        request.push(("DBClusterParameterGroupName", group.to_string()));
    }
    let created = the_cluster(
        identifier,
        &expect_ok(identifier, call(router, "CreateDBCluster", &request).await),
    );
    let address = text(&created, "Endpoint");
    let port: u16 = text(&created, "Port")
        .parse()
        .unwrap_or_else(|_| panic!("case {identifier:?}: the cluster publishes a port"));
    wait_for_settle();
    (address, port)
}

/// A MySQL session on the cluster's own endpoint, as its master user.
async fn session(address: &str, port: u16) -> MySqlSession {
    mysql_login(
        address,
        port,
        Transport::Plaintext,
        MASTER_USER,
        MASTER_PASSWORD,
        Some(DATABASE),
    )
    .await
    .unwrap_or_else(|refusal| panic!("the master user must be admitted, got {refusal:?}"))
}

/// Run `sql`, failing the case when the engine refuses it.
async fn run(session: &mut MySqlSession, case: &str, sql: &str) {
    session
        .query(sql)
        .await
        .unwrap_or_else(|refusal| panic!("case {case:?}: {sql:?} must run, got {refusal:?}"));
}

/// The rows the table `table` holds, as text, in the order the engine returns them.
async fn rows_of(session: &mut MySqlSession, case: &str, table: &str) -> Vec<Vec<Option<String>>> {
    session
        .query(&format!("SELECT * FROM {table}"))
        .await
        .unwrap_or_else(|refusal| panic!("case {case:?}: the read-back must run, got {refusal:?}"))
        .rows
}

/// One row of the format table: the object(s) staged, the statement that loads them, and the
/// rows the table must hold afterwards.
struct LoadRow {
    case: &'static str,
    table: &'static str,
    /// The table's declaration, so each row spans the columns its own object carries.
    declaration: &'static str,
    /// The objects staged for this row, as `(key, body)`.
    objects: &'static [(&'static str, &'static str)],
    /// The `LOAD DATA FROM S3` statement, with `{table}` and `{uri}` filled in from the row.
    statement: &'static str,
    /// The URI the statement names, relative to the bucket.
    uri_key: &'static str,
    expected: &'static [&'static [&'static str]],
}

/// The format table: the rule generalises over which objects the URI names, how the records
/// are delimited, which columns they carry and which records are skipped, so the rows differ
/// along exactly those.
///
/// The clause names and the three sources are the statement's own grammar, as the Aurora
/// MySQL `LOAD DATA FROM S3` reference states it (`FILE`, `PREFIX`, `MANIFEST`,
/// `FIELDS TERMINATED BY`, `[OPTIONALLY] ENCLOSED BY`, `LINES TERMINATED BY`,
/// `IGNORE <n> LINES`, and the column list).
const LOAD_ROWS: &[LoadRow] = &[
    LoadRow {
        case: "a comma-separated object, the form a client writes a CSV export as",
        table: "people",
        declaration: "CREATE TABLE people (id int, name varchar(32))",
        objects: &[("import/rows.csv", "1,alice\n2,bob\n3,carol\n")],
        statement: "LOAD DATA FROM S3 '{uri}' INTO TABLE {table} FIELDS TERMINATED BY ','",
        uri_key: "import/rows.csv",
        expected: &[&["1", "alice"], &["2", "bob"], &["3", "carol"]],
    },
    LoadRow {
        case: "the default field format, which is tab-separated",
        table: "tabbed",
        declaration: "CREATE TABLE tabbed (id int, name varchar(32))",
        objects: &[("import/rows.tsv", "7\tgamma\n8\tdelta\n")],
        statement: "LOAD DATA FROM S3 '{uri}' INTO TABLE {table}",
        uri_key: "import/rows.tsv",
        expected: &[&["7", "gamma"], &["8", "delta"]],
    },
    LoadRow {
        case: "a header row the statement skips",
        table: "headed",
        declaration: "CREATE TABLE headed (id int, name varchar(32))",
        objects: &[("import/headed.csv", "id,name\n4,dave\n5,erin\n")],
        statement: "LOAD DATA FROM S3 '{uri}' INTO TABLE {table} FIELDS TERMINATED BY ',' \
                    IGNORE 1 LINES",
        uri_key: "import/headed.csv",
        expected: &[&["4", "dave"], &["5", "erin"]],
    },
    LoadRow {
        case: "quoted fields carrying the delimiter, and an escaped NULL marker",
        table: "quoted",
        declaration: "CREATE TABLE quoted (id int, name varchar(32))",
        objects: &[(
            "import/quoted.csv",
            "1,\"last, first\"\n2,\\N\n",
        )],
        statement: "LOAD DATA FROM S3 '{uri}' INTO TABLE {table} FIELDS TERMINATED BY ',' \
                    OPTIONALLY ENCLOSED BY '\"'",
        uri_key: "import/quoted.csv",
        // The second row's name is the NULL marker, so the column is NULL rather than text.
        expected: &[&["1", "last, first"], &["2", "\0NULL"]],
    },
    LoadRow {
        case: "a column list naming the columns in another order than the table declares",
        table: "reordered",
        declaration: "CREATE TABLE reordered (id int, name varchar(32))",
        objects: &[("import/reordered.csv", "zoe,9\n")],
        statement: "LOAD DATA FROM S3 '{uri}' INTO TABLE {table} FIELDS TERMINATED BY ',' \
                    (name, id)",
        uri_key: "import/reordered.csv",
        expected: &[&["9", "zoe"]],
    },
    LoadRow {
        case: "a PREFIX naming every object under it, loaded in key order",
        table: "prefixed",
        declaration: "CREATE TABLE prefixed (id int, name varchar(32))",
        objects: &[
            ("batch/part-1.csv", "10,one\n"),
            ("batch/part-2.csv", "11,two\n"),
            // Outside the prefix, so this object's row must NOT be loaded.
            ("other/part-3.csv", "12,three\n"),
        ],
        statement: "LOAD DATA FROM S3 PREFIX '{uri}' INTO TABLE {table} FIELDS TERMINATED BY ','",
        uri_key: "batch/",
        expected: &[&["10", "one"], &["11", "two"]],
    },
    LoadRow {
        case: "a MANIFEST naming the objects to load",
        table: "manifested",
        declaration: "CREATE TABLE manifested (id int, name varchar(32))",
        objects: &[
            ("manifest/rows-a.csv", "20,twenty\n"),
            ("manifest/rows-b.csv", "21,twentyone\n"),
            (
                "manifest/load.json",
                "{\"entries\":[\
                   {\"url\":\"s3://emu-rds-s3-import/manifest/rows-a.csv\",\"mandatory\":true},\
                   {\"url\":\"s3://emu-rds-s3-import/manifest/rows-b.csv\"}]}",
            ),
        ],
        statement: "LOAD DATA FROM S3 MANIFEST '{uri}' INTO TABLE {table} FIELDS TERMINATED BY ','",
        uri_key: "manifest/load.json",
        expected: &[&["20", "twenty"], &["21", "twentyone"]],
    },
    LoadRow {
        case: "a table name qualified by the database it is in",
        table: "qualified",
        declaration: "CREATE TABLE qualified (id int, name varchar(32))",
        objects: &[("import/qualified.csv", "30,thirty\n")],
        // The qualifier is the database every session in this suite is on, which is where
        // the table was declared.
        statement: "LOAD DATA FROM S3 '{uri}' INTO TABLE appdb.{table} FIELDS TERMINATED BY ','",
        uri_key: "import/qualified.csv",
        expected: &[&["30", "thirty"]],
    },
    LoadRow {
        case: "IGNORE keeps the row already stored under a duplicate key",
        table: "ignored",
        declaration: "CREATE TABLE ignored (id int PRIMARY KEY, name varchar(32))",
        objects: &[("import/duplicates.csv", "40,first\n40,second\n41,other\n")],
        statement: "LOAD DATA FROM S3 '{uri}' IGNORE INTO TABLE {table} FIELDS TERMINATED BY ','",
        uri_key: "import/duplicates.csv",
        expected: &[&["40", "first"], &["41", "other"]],
    },
    LoadRow {
        case: "REPLACE puts the incoming row in its place instead",
        table: "replaced",
        declaration: "CREATE TABLE replaced (id int PRIMARY KEY, name varchar(32))",
        objects: &[("import/duplicates.csv", "40,first\n40,second\n41,other\n")],
        statement: "LOAD DATA FROM S3 '{uri}' REPLACE INTO TABLE {table} FIELDS TERMINATED BY ','",
        uri_key: "import/duplicates.csv",
        expected: &[&["40", "second"], &["41", "other"]],
    },
];

/// The marker a row's expected value carries where the column must be SQL NULL, which no CSV
/// field in this suite can spell.
const NULL_MARKER: &str = "\0NULL";

/// The expected rows of one table row, as the wire reports them.
fn expected_rows(row: &LoadRow) -> Vec<Vec<Option<String>>> {
    row.expected
        .iter()
        .map(|values| {
            values
                .iter()
                .map(|value| {
                    (*value != NULL_MARKER).then(|| (*value).to_string())
                })
                .collect()
        })
        .collect()
}

/// Set up the cluster whose import role is configured both ways AWS asks for: the role is
/// associated with the cluster and the cluster's parameter group names it.
async fn importing_cluster(
    router: &MultiServiceRouter,
    identifier: &str,
    group_name: &str,
) -> (String, u16) {
    let role_arn = role(router, "rds-s3-load-role", st::RDS_SERVICE_PRINCIPAL, None).await;
    let group = create_group(router, identifier, group_name, "aurora-mysql8.0").await;
    expect_ok(
        identifier,
        modify(router, &group, "aws_default_s3_role", &role_arn, None).await,
    );
    let published = available_cluster(router, identifier, "aurora-mysql", Some(&group)).await;
    // Associated under NO feature name, which is how an Aurora MySQL cluster takes the role
    // that serves its Amazon S3 integration.
    expect_ok(
        identifier,
        call(
            router,
            "AddRoleToDBCluster",
            &[
                ("DBClusterIdentifier", identifier),
                ("RoleArn", role_arn.as_str()),
            ],
        )
        .await,
    );
    published
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_s3_import_loads_the_objects_own_records_as_rows() {
    let (router, _engine) = emulator();
    create_bucket(&router, BUCKET, REGION).await;
    for row in LOAD_ROWS {
        for (key, body) in row.objects {
            put_object(&router, BUCKET, key, body.as_bytes().to_vec()).await;
        }
    }
    let (address, port) = importing_cluster(&router, "emu-s3-load", "emu-s3-load-cpg").await;
    let mut session = session(&address, port).await;

    for row in LOAD_ROWS {
        run(&mut session, row.case, row.declaration).await;
        let statement = row
            .statement
            .replace("{table}", row.table)
            .replace("{uri}", &format!("s3://{BUCKET}/{}", row.uri_key));
        session
            .query(&statement)
            .await
            .unwrap_or_else(|refusal| {
                panic!("case {:?}: {statement:?} must load, got {refusal:?}", row.case)
            });
        let loaded = rows_of(&mut session, row.case, row.table).await;
        assert_eq!(
            loaded,
            expected_rows(row),
            "case {:?}: expected the object's own rows, got {loaded:?}",
            row.case
        );
    }
}

/// One row of the object-read table: what the statement names, and the sentence the engine's
/// own Amazon S3 call fails with.
struct MissingObjectRow {
    case: &'static str,
    uri: &'static str,
}

/// The URIs the statement can name that Amazon S3 resolves no object for: a key the bucket
/// has not got, a prefix nothing lies under, a bucket that does not exist, and a URI that is
/// no Amazon S3 URI at all. The engine learns the same thing about each — its read answered
/// with no object — so each carries the same S3 API error.
const MISSING_OBJECT_ROWS: &[MissingObjectRow] = &[
    MissingObjectRow {
        case: "a key the bucket does not hold",
        uri: "s3://emu-rds-s3-import/import/absent.csv",
    },
    MissingObjectRow {
        case: "a prefix nothing lies under",
        uri: "s3://emu-rds-s3-import/absent-prefix/",
    },
    MissingObjectRow {
        case: "a bucket Amazon S3 does not hold",
        uri: "s3://emu-no-such-bucket/import/rows.csv",
    },
    MissingObjectRow {
        case: "a URI that is no Amazon S3 URI",
        uri: "https://example.invalid/rows.csv",
    },
];

/// The sentence the engine reports when its Amazon S3 read answers with no object.
const S3_NOT_FOUND: &str = "S3 API returned error: Not Found:Not Found";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_object_amazon_s3_does_not_hold_fails_the_engines_own_s3_call() {
    let (router, _engine) = emulator();
    create_bucket(&router, BUCKET, REGION).await;
    let (address, port) = importing_cluster(&router, "emu-s3-missing", "emu-s3-missing-cpg").await;
    let mut session = session(&address, port).await;
    run(
        &mut session,
        "missing objects",
        "CREATE TABLE landing (id int, name varchar(32))",
    )
    .await;
    for row in MISSING_OBJECT_ROWS {
        let prefix = if row.uri.ends_with('/') { "PREFIX " } else { "" };
        let statement = format!(
            "LOAD DATA FROM S3 {prefix}'{}' INTO TABLE landing FIELDS TERMINATED BY ','",
            row.uri
        );
        let refusal = session
            .query(&statement)
            .await
            .expect_err(row.case);
        assert_eq!(
            (refusal.code.as_str(), refusal.message.as_str()),
            (ER_S3_API_ERROR, S3_NOT_FOUND),
            "case {:?}: expected the server's S3 API error, got {refusal:?}",
            row.case
        );
        let loaded = rows_of(&mut session, row.case, "landing").await;
        assert!(
            loaded.is_empty(),
            "case {:?}: a load that failed must leave no row, got {loaded:?}",
            row.case
        );
    }
}

/// One step of the configuration table: what the cluster has been configured with so far,
/// and whether the engine can then reach Amazon S3.
struct ConfigurationStep {
    case: &'static str,
    /// Whether this step associates the role the parameter names.
    associate_named_role: bool,
    /// Whether this step associates ANOTHER role, which names no import role.
    associate_other_role: bool,
    /// Whether this step sets the import-role parameter to the named role.
    set_parameter: bool,
    /// Whether the load then succeeds.
    loads: bool,
}

/// The configuration table: an Amazon S3 import needs BOTH halves of what AWS asks for — the
/// role associated with the cluster and an import-role parameter naming it — so the steps
/// span each half on its own and then both, on ONE cluster and with ONE statement, which is
/// what pins the refusal on the configuration rather than on the grammar.
const CONFIGURATION_STEPS: &[ConfigurationStep] = &[
    ConfigurationStep {
        case: "neither the association nor the parameter",
        associate_named_role: false,
        associate_other_role: false,
        set_parameter: false,
        loads: false,
    },
    ConfigurationStep {
        case: "the parameter names a role the cluster has not associated",
        associate_named_role: false,
        associate_other_role: false,
        set_parameter: true,
        loads: false,
    },
    ConfigurationStep {
        case: "a role is associated, but no import-role parameter names it",
        associate_named_role: false,
        associate_other_role: true,
        set_parameter: true,
        loads: false,
    },
    ConfigurationStep {
        case: "the parameter names the role the cluster has associated",
        associate_named_role: true,
        associate_other_role: false,
        set_parameter: true,
        loads: true,
    },
];

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_import_needs_the_associated_role_its_parameter_names() {
    let (router, _engine) = emulator();
    create_bucket(&router, BUCKET, REGION).await;
    put_object(
        &router,
        BUCKET,
        "import/rows.csv",
        b"1,alice\n2,bob\n".to_vec(),
    )
    .await;
    let identifier = "emu-s3-role-gate";
    let import_role = role(&router, "rds-s3-gate-import", st::RDS_SERVICE_PRINCIPAL, None).await;
    let other_role = role(&router, "rds-s3-gate-other", st::RDS_SERVICE_PRINCIPAL, None).await;
    let group = create_group(&router, identifier, "emu-s3-gate-cpg", "aurora-mysql8.0").await;
    let (address, port) =
        available_cluster(&router, identifier, "aurora-mysql", Some(&group)).await;
    let mut session = session(&address, port).await;
    run(
        &mut session,
        "the role gate",
        "CREATE TABLE landing (id int, name varchar(32))",
    )
    .await;
    let statement = format!(
        "LOAD DATA FROM S3 's3://{BUCKET}/import/rows.csv' INTO TABLE landing \
         FIELDS TERMINATED BY ','"
    );

    for step in CONFIGURATION_STEPS {
        if step.set_parameter {
            expect_ok(
                step.case,
                modify(&router, &group, "aws_default_s3_role", &import_role, None).await,
            );
        }
        for (associate, role_arn) in [
            (step.associate_other_role, &other_role),
            (step.associate_named_role, &import_role),
        ] {
            if associate {
                expect_ok(
                    step.case,
                    call(
                        &router,
                        "AddRoleToDBCluster",
                        &[
                            ("DBClusterIdentifier", identifier),
                            ("RoleArn", role_arn.as_str()),
                        ],
                    )
                    .await,
                );
            }
        }
        let answer = session.query(&statement).await;
        if step.loads {
            answer.unwrap_or_else(|refusal| {
                panic!(
                    "case {:?}: the same statement must load once the role is configured, got \
                     {refusal:?}",
                    step.case
                )
            });
            let loaded = rows_of(&mut session, step.case, "landing").await;
            assert_eq!(
                loaded,
                vec![
                    vec![Some("1".to_string()), Some("alice".to_string())],
                    vec![Some("2".to_string()), Some("bob".to_string())],
                ],
                "case {:?}: expected the object's own rows, got {loaded:?}",
                step.case
            );
            continue;
        }
        let refusal: Refusal = answer.expect_err(step.case);
        assert_eq!(
            (refusal.code.as_str(), refusal.message.as_str()),
            (
                ER_S3_API_ERROR,
                "S3 API returned error: Missing Credentials: Cannot instantiate S3 Client"
            ),
            "case {:?}: the statement must parse and fail where the engine reaches Amazon S3, \
             got {refusal:?}",
            step.case
        );
        assert_ne!(
            refusal.code, ER_PARSE_ERROR,
            "case {:?}: a missing import role must not look like a statement the engine has \
             not got",
            step.case
        );
        let loaded = rows_of(&mut session, step.case, "landing").await;
        assert!(
            loaded.is_empty(),
            "case {:?}: a load the engine could not perform must leave no row, got {loaded:?}",
            step.case
        );
    }
}

/// One row of the malformed-statement table: what the client sent, and the server error
/// number it must be refused under.
struct MalformedRow {
    case: &'static str,
    /// The statement, with `{uri}` filled in from the bucket.
    statement: &'static str,
    code: &'static str,
}

/// The malformed-statement table: a statement that begins `LOAD DATA FROM S3` and then does
/// not fit what this endpoint can read is refused — truncated after each of the grammar's own
/// positions, with a format value the reader cannot honour, with a database qualifier the
/// resource has not got, with a manifest whose bytes are not the document the form takes, and
/// longer than this endpoint serves — and nothing is loaded by any of them.
const MALFORMED_ROWS: &[MalformedRow] = &[
    MalformedRow {
        case: "truncated after the source keyword",
        statement: "LOAD DATA FROM S3",
        code: ER_PARSE_ERROR,
    },
    MalformedRow {
        case: "truncated after the URI",
        statement: "LOAD DATA FROM S3 '{uri}'",
        code: ER_PARSE_ERROR,
    },
    MalformedRow {
        case: "truncated after INTO TABLE",
        statement: "LOAD DATA FROM S3 '{uri}' INTO TABLE",
        code: ER_PARSE_ERROR,
    },
    MalformedRow {
        case: "a FIELDS clause naming none of its own parts",
        statement: "LOAD DATA FROM S3 '{uri}' INTO TABLE landing FIELDS",
        code: ER_PARSE_ERROR,
    },
    MalformedRow {
        case: "a LINES clause whose part carries no string",
        statement: "LOAD DATA FROM S3 '{uri}' INTO TABLE landing LINES TERMINATED BY",
        code: ER_PARSE_ERROR,
    },
    MalformedRow {
        case: "an IGNORE whose count is not a number",
        statement: "LOAD DATA FROM S3 '{uri}' INTO TABLE landing IGNORE many LINES",
        code: ER_PARSE_ERROR,
    },
    MalformedRow {
        case: "a column list that is not closed",
        statement: "LOAD DATA FROM S3 '{uri}' INTO TABLE landing (id, name",
        code: ER_PARSE_ERROR,
    },
    MalformedRow {
        case: "a MANIFEST whose object is not the document the form takes",
        statement: "LOAD DATA FROM S3 MANIFEST 's3://emu-rds-s3-import/import/rows.csv' \
                    INTO TABLE landing",
        code: ER_S3_API_ERROR,
    },
    // A format value this reader cannot honour is REFUSED rather than cut down to something
    // the statement did not name: an enclosing or escape character has to be one byte, and a
    // field or line terminator likewise.
    MalformedRow {
        case: "an ENCLOSED BY of more than one character",
        statement: "LOAD DATA FROM S3 '{uri}' INTO TABLE landing FIELDS TERMINATED BY ',' \
                    ENCLOSED BY 'xy'",
        code: ER_NOT_SUPPORTED_YET,
    },
    MalformedRow {
        case: "a multi-byte ESCAPED BY character",
        statement: "LOAD DATA FROM S3 '{uri}' INTO TABLE landing FIELDS TERMINATED BY ',' \
                    ESCAPED BY 'é'",
        code: ER_NOT_SUPPORTED_YET,
    },
    MalformedRow {
        case: "a multi-character LINES TERMINATED BY that is not a CRLF",
        statement: "LOAD DATA FROM S3 '{uri}' INTO TABLE landing LINES TERMINATED BY '||'",
        code: ER_NOT_SUPPORTED_YET,
    },
    MalformedRow {
        case: "a table qualified by a database the resource has not got",
        statement: "LOAD DATA FROM S3 '{uri}' INTO TABLE nosuchdb.landing \
                    FIELDS TERMINATED BY ','",
        code: ER_BAD_DB_ERROR,
    },
];

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_statement_outside_the_grammar_is_refused_and_loads_nothing() {
    let (router, _engine) = emulator();
    create_bucket(&router, BUCKET, REGION).await;
    put_object(&router, BUCKET, "import/rows.csv", b"1,alice\n".to_vec()).await;
    let (address, port) =
        importing_cluster(&router, "emu-s3-malformed", "emu-s3-malformed-cpg").await;
    let mut session = session(&address, port).await;
    run(
        &mut session,
        "malformed statements",
        "CREATE TABLE landing (id int, name varchar(32))",
    )
    .await;

    let uri = format!("s3://{BUCKET}/import/rows.csv");
    for row in MALFORMED_ROWS {
        let statement = row.statement.replace("{uri}", &uri);
        let refusal = session.query(&statement).await.expect_err(row.case);
        assert_eq!(
            refusal.code, row.code,
            "case {:?}: expected the statement to be refused under {}, got {refusal:?}",
            row.case, row.code
        );
        let loaded = rows_of(&mut session, row.case, "landing").await;
        assert!(
            loaded.is_empty(),
            "case {:?}: a refused statement must load nothing, got {loaded:?}",
            row.case
        );
    }

    // A statement longer than this endpoint serves is refused by the session's own limit
    // rather than read: the URI below is far past it.
    let oversized = format!(
        "LOAD DATA FROM S3 's3://{BUCKET}/{}' INTO TABLE landing",
        "a".repeat(st::data_plane::wire_session::MAX_STATEMENT_BYTES)
    );
    let refusal = session
        .query(&oversized)
        .await
        .expect_err("a statement past the endpoint's limit");
    assert_eq!(
        refusal.code, ER_PARSE_ERROR,
        "a statement past the endpoint's limit must be refused, got {refusal:?}"
    );
    let loaded = rows_of(&mut session, "an oversized statement", "landing").await;
    assert!(
        loaded.is_empty(),
        "an oversized statement must load nothing, got {loaded:?}"
    );
}

/// A DB INSTANCE of `engine` with `group` as its DB parameter group, waited out and dialled.
async fn available_instance(
    router: &MultiServiceRouter,
    identifier: &str,
    engine: &str,
    group: Option<&str>,
) -> (String, u16) {
    let mut request: Vec<(&str, String)> = vec![
        ("DBInstanceIdentifier", identifier.to_string()),
        ("Engine", engine.to_string()),
        ("DBInstanceClass", "db.t3.micro".to_string()),
        ("AllocatedStorage", "20".to_string()),
        ("MasterUsername", MASTER_USER.to_string()),
        ("MasterUserPassword", MASTER_PASSWORD.to_string()),
        ("DBName", DATABASE.to_string()),
    ];
    if let Some(group) = group {
        request.push(("DBParameterGroupName", group.to_string()));
    }
    let created = expect_ok(identifier, call(router, "CreateDBInstance", &request).await);
    let endpoint = endpoint_of(&created);
    wait_for_settle();
    endpoint
}

/// Rule, on the OTHER resource the rule covers: an RDS for MySQL DB INSTANCE reads the same
/// statement and resolves the import role from the same parameters — its own DB PARAMETER
/// group's — so with the parameter set but no role associated the statement parses and fails
/// at execution rather than being rejected as a syntax error.
///
/// The positive instance case is not reachable here: an association on a DB instance names a
/// `FeatureName`, and the engine-version catalogue publishes none for RDS for MySQL, so no
/// role can be associated with such an instance at all — that is the subject of the open
/// issue 20261107T185637Z-rds-s3-integration-3-causes-no-aws-s3.md, which owns the
/// instance-side association causes. What this pins is the half that is this fix's: the
/// statement is READ on that engine and answers the engine's own S3 condition.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_rds_for_mysql_instance_reads_the_statement_and_answers_its_own_s3_condition() {
    let (router, _engine) = emulator();
    create_bucket(&router, BUCKET, REGION).await;
    put_object(&router, BUCKET, "import/rows.csv", b"1,alice\n".to_vec()).await;
    let role_arn = role(&router, "rds-s3-instance-role", st::RDS_SERVICE_PRINCIPAL, None).await;
    // The instance-level counterpart of the cluster's group: the same import-role parameter,
    // in the parameter group an instance is configured by.
    parameter_group(
        &router,
        "emu-s3-instance-pg",
        "mysql8.0",
        "aws_default_s3_role",
        &role_arn,
    )
    .await;
    let (address, port) = available_instance(
        &router,
        "emu-s3-instance",
        "mysql",
        Some("emu-s3-instance-pg"),
    )
    .await;
    let mut session = session(&address, port).await;
    run(
        &mut session,
        "an RDS for MySQL instance",
        "CREATE TABLE landing (id int, name varchar(32))",
    )
    .await;
    let refusal = session
        .query(&format!(
            "LOAD DATA FROM S3 's3://{BUCKET}/import/rows.csv' INTO TABLE landing \
             FIELDS TERMINATED BY ','"
        ))
        .await
        .expect_err("no role is associated with the instance");
    assert_eq!(
        (refusal.code.as_str(), refusal.message.as_str()),
        (
            ER_S3_API_ERROR,
            "S3 API returned error: Missing Credentials: Cannot instantiate S3 Client"
        ),
        "the instance must read the statement and answer the engine's own S3 condition, got \
         {refusal:?}"
    );
    assert_ne!(
        refusal.code, ER_PARSE_ERROR,
        "the statement must not be answered as one the engine's grammar has not got"
    );
    let loaded = rows_of(&mut session, "an RDS for MySQL instance", "landing").await;
    assert!(
        loaded.is_empty(),
        "a load the engine could not perform must leave no row, got {loaded:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_engine_whose_grammar_has_no_s3_source_answers_its_own_parse_error() {
    let (router, _engine) = emulator();
    create_bucket(&router, BUCKET, REGION).await;
    put_object(&router, BUCKET, "import/rows.csv", b"1,alice\n".to_vec()).await;
    // MariaDB speaks the same wire as MySQL and has no `FROM S3` source in its `LOAD DATA`,
    // so the statement is not one its engine defines.
    let (address, port) = available_instance(&router, "emu-s3-mariadb", "mariadb", None).await;
    let mut session = session(&address, port).await;
    run(
        &mut session,
        "mariadb",
        "CREATE TABLE landing (id int, name varchar(32))",
    )
    .await;
    let refusal = session
        .query(&format!(
            "LOAD DATA FROM S3 's3://{BUCKET}/import/rows.csv' INTO TABLE landing"
        ))
        .await
        .expect_err("MariaDB has no FROM S3 source");
    assert_eq!(
        refusal.code, ER_PARSE_ERROR,
        "an engine whose grammar has no S3 source must answer its own parse error, got \
         {refusal:?}"
    );
}
