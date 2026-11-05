// SPDX-License-Identifier: CC0-1.0

//! What the catalog may claim as an IDENTITY — the `QuotaCode` of a quota and the
//! `ServiceName` of a service — over the wire.
//!
//! Both are identifiers AWS owns, and neither is derivable from the model, so the
//! rules asserted here are about what this emulator is allowed to publish rather
//! than about one recorded value:
//!
//! * `L-` followed by eight hex characters is the shape of a code AWS MINTS. A code
//!   of that shape is what a client reads out of AWS's published catalog and looks a
//!   quota up by, so every one this catalog publishes must be a code it cites from
//!   AWS; a quota whose AWS code the catalog cannot cite is published under a
//!   descriptive code built from the quota's own published name;
//! * `ServiceInfo$ServiceName` is the service's display name, which is a different
//!   string from its `ServiceCode` — a service listed under its own code as its name
//!   renders a name that never appears in AWS;
//! * a code in AWS's namespace that this catalog does not cite resolves nothing, so
//!   a client holding a real AWS quota code never reads a quota back from one this
//!   emulator made up.
//!
//! Each suite drives a TABLE of inputs through `ListServices`,
//! `ListAWSDefaultServiceQuotas` and `GetAWSDefaultServiceQuota`, and the whole
//! catalog where the rule is an invariant over it. The harness is the one
//! `quota_workflow.rs` owns.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_servicequotas_servicequotasv20190624::state::descriptive_quota_code;

// The catalog walks (`inventory`, `defaults`, `whole_catalog`) live in the shared
// harness, so this suite and `default_quota_catalog_fidelity.rs` share one copy.
use crate::quota_workflow::{
    client, default_quotas as defaults, emulator, service_inventory as inventory,
    whole_default_catalog as whole_catalog,
};

/// The `(ServiceCode, QuotaCode)` pairs this catalog CITES from AWS's own published
/// Service Quotas data — the only quotas allowed to carry a code in AWS's minted
/// `L-` namespace.
///
/// SOURCE: the `.code("L-…")` rows of `src/state.rs::seed_quota_catalog` (each one
/// carries AWS's code for that quota). A new row that claims an `L-` code and is not
/// listed here fails `only_a_cited_quota_code_sits_in_the_aws_minted_namespace`,
/// which is the point: a code in that namespace has to be attributable.
const CITED_AWS_QUOTA_CODES: &[(&str, &str)] = &[
    ("access-analyzer", "L-1E51937C"),
    ("access-analyzer", "L-5EFCE71D"),
    ("access-analyzer", "L-96F55078"),
    ("account-access", "L-42A1D874"),
    ("acm", "L-DD60D132"),
    ("acm-pca", "L-01B8608D"),
    ("aco-automation", "L-1501440B"),
    ("agent-registry", "L-7F384C37"),
    ("aidevops", "L-4FF187E6"),
    ("aiops", "L-EF31E0C4"),
    ("airflow", "L-52DCDA32"),
    ("airflow", "L-67D41C6B"),
    ("apigateway", "L-01C8A9E0"),
    ("apigateway", "L-05295E0B"),
    ("apigateway", "L-154598D1"),
    ("apigateway", "L-1D180A63"),
    ("apigateway", "L-20859C74"),
    ("apigateway", "L-24E7E662"),
    ("apigateway", "L-379E48B0"),
    ("apigateway", "L-3A613F94"),
    ("apigateway", "L-45CC0E8B"),
    ("apigateway", "L-46624B39"),
    ("apigateway", "L-4D98A8A5"),
    ("apigateway", "L-5244589D"),
    ("apigateway", "L-608BDCD4"),
    ("apigateway", "L-60AC41CD"),
    ("apigateway", "L-65B5C802"),
    ("apigateway", "L-668C9B28"),
    ("apigateway", "L-68B79FF0"),
    ("apigateway", "L-7D4D47CD"),
    ("apigateway", "L-824C9E42"),
    ("apigateway", "L-85120D8E"),
    ("apigateway", "L-8A5B8E43"),
    ("apigateway", "L-8B81B02C"),
    ("apigateway", "L-8C2F9A1D"),
    ("apigateway", "L-8E6A5A87"),
    ("apigateway", "L-8F8AAAFE"),
    ("apigateway", "L-95BA6EA5"),
    ("apigateway", "L-985EB478"),
    ("apigateway", "L-9C147DE4"),
    ("apigateway", "L-9ED1E49A"),
    ("apigateway", "L-A4C7274F"),
    ("apigateway", "L-A6CCE716"),
    ("apigateway", "L-A7033131"),
    ("apigateway", "L-A93447B8"),
    ("apigateway", "L-A966AB5C"),
    ("apigateway", "L-A9DBC573"),
    ("apigateway", "L-AA0FF27B"),
    ("apigateway", "L-B2CF62DC"),
    ("apigateway", "L-B4D69DB7"),
    ("apigateway", "L-B97207D0"),
    ("apigateway", "L-CAEF46D2"),
    ("apigateway", "L-CC2525B6"),
    ("apigateway", "L-CDF5615A"),
    ("apigateway", "L-E11F7D5D"),
    ("apigateway", "L-E1465507"),
    ("apigateway", "L-E5AE38E3"),
    ("apigateway", "L-E8693075"),
    ("apigateway", "L-E9EEB922"),
    ("apigateway", "L-F8A34CAF"),
    ("apigateway", "L-F8BD84D3"),
    ("apigateway", "L-FB4F0270"),
    ("apigateway", "L-FD0EB744"),
    ("appconfig", "L-08D2C632"),
    ("appconfig", "L-48F9B951"),
    ("appconfig", "L-A52E46BE"),
    ("appconfig", "L-A5FC0339"),
    ("appconfig", "L-B858C085"),
    ("appconfig", "L-EEB0151E"),
    ("appconfig", "L-F59D302B"),
    ("appconfig", "L-FA210A1F"),
    ("application-autoscaling", "L-17D5F681"),
    ("application-autoscaling", "L-1A11EB4B"),
    ("application-autoscaling", "L-1AAF0700"),
    ("application-autoscaling", "L-297D9EC9"),
    ("application-autoscaling", "L-4A11ECEB"),
    ("application-autoscaling", "L-60E4E5E2"),
    ("application-autoscaling", "L-782A3EE2"),
    ("application-autoscaling", "L-799ACCDF"),
    ("application-autoscaling", "L-800F6F7B"),
    ("application-autoscaling", "L-8998F403"),
    ("application-autoscaling", "L-95848B5F"),
    ("application-autoscaling", "L-9C25247C"),
    ("application-autoscaling", "L-A1D42901"),
    ("application-autoscaling", "L-A2AD6458"),
    ("application-autoscaling", "L-B395C81B"),
    ("application-autoscaling", "L-D75CA9D2"),
    ("appmesh", "L-33E8F9C9"),
    ("appmesh", "L-50F6C35A"),
    ("appmesh", "L-606A910B"),
    ("appmesh", "L-8775AB18"),
    ("appmesh", "L-87E74146"),
    ("appmesh", "L-AC861A39"),
    ("appmesh", "L-AE1D9567"),
    ("appmesh", "L-BB90B7FF"),
    ("appmesh", "L-DA7495A7"),
    ("appmesh", "L-E043DFB4"),
    ("appmesh", "L-F6F26D09"),
    ("apprunner", "L-1BDBAAB6"),
    ("apprunner", "L-1D8C5BDD"),
    ("apprunner", "L-4064914D"),
    ("apprunner", "L-69F96A0C"),
    ("apprunner", "L-A0B46A0C"),
    ("apprunner", "L-F7ADEB8C"),
    ("athena", "L-076245C0"),
    ("athena", "L-07F4444F"),
    ("athena", "L-0F303755"),
    ("athena", "L-136E993A"),
    ("athena", "L-3702C1C1"),
    ("athena", "L-3CE0BBA0"),
    ("athena", "L-41075247"),
    ("athena", "L-4FF11DD3"),
    ("athena", "L-52EFA794"),
    ("athena", "L-56A94400"),
    ("athena", "L-5A8D5237"),
    ("athena", "L-70FD91F7"),
    ("athena", "L-722F8D5C"),
    ("athena", "L-76AFE6F2"),
    ("athena", "L-7A12A1E6"),
    ("athena", "L-82E47245"),
    ("athena", "L-9D86AA19"),
    ("athena", "L-A6257554"),
    ("athena", "L-AC64E3A0"),
    ("athena", "L-B13EED8E"),
    ("athena", "L-C60BEAE0"),
    ("athena", "L-CC64F7CE"),
    ("athena", "L-D0D8C912"),
    ("athena", "L-D8C0E3AF"),
    ("athena", "L-E20AD6B8"),
    ("athena", "L-E80DC288"),
    ("athena", "L-EFB619E3"),
    ("athena", "L-FC5F6546"),
    ("athena", "L-FD9D80C2"),
    ("athena", "L-FE3693EE"),
    ("autoscaling", "L-05CB8B12"),
    ("autoscaling", "L-1312BBBF"),
    ("autoscaling", "L-6B80B8FA"),
    ("autoscaling", "L-6C2A2F6E"),
    ("autoscaling", "L-72753F6F"),
    ("autoscaling", "L-CDE20ADC"),
    ("autoscaling", "L-CEE5E714"),
    ("autoscaling", "L-F0B00D71"),
    ("autoscaling", "L-F786B2E5"),
    ("backup", "L-366B61FD"),
    ("backup", "L-514878B6"),
    ("backup", "L-7705D2CB"),
    ("backup", "L-9122A82F"),
    ("backup", "L-9CAF0B92"),
    ("backup", "L-B4021FB0"),
    ("backup", "L-BD69F607"),
    ("backup", "L-C0A8C14B"),
    ("backup", "L-C296F1F5"),
    ("backup", "L-E43E0ED6"),
    ("backup", "L-FB1D55EC"),
    ("backup", "L-FFD6444F"),
    ("codecommit", "L-81790602"),
    ("cognito-idp", "L-7CDAF993"),
    ("config", "L-0BE82991"),
    ("dynamodb", "L-F98FE922"),
    ("ec2", "L-0263D0A3"),
    ("ec2", "L-1216C47A"),
    ("ec2", "L-34B43A08"),
    ("events", "L-244521F2"),
    ("events", "L-388D1D08"),
    ("events", "L-39263BDC"),
    ("events", "L-3C47459F"),
    ("events", "L-5425B7E9"),
    ("events", "L-5540C5E3"),
    ("events", "L-595D6D42"),
    ("events", "L-5D1D13E3"),
    ("events", "L-658A4FD9"),
    ("events", "L-664C5505"),
    ("events", "L-755FD01C"),
    ("events", "L-8331B29E"),
    ("events", "L-9B653E91"),
    ("events", "L-A8AFA624"),
    ("events", "L-CB72B930"),
    ("events", "L-D082A4F6"),
    ("events", "L-DF788473"),
    ("events", "L-E2110C81"),
    ("events", "L-EAC9A2AC"),
    ("events", "L-FB1C3A6D"),
    ("events", "L-FC354966"),
    ("firehose", "L-040D05FF"),
    ("firehose", "L-09E8B729"),
    ("firehose", "L-0D32C2BC"),
    ("firehose", "L-14BB0BE7"),
    ("firehose", "L-17D4EADE"),
    ("firehose", "L-23F03908"),
    ("firehose", "L-36A7E04C"),
    ("firehose", "L-3A800C3A"),
    ("firehose", "L-47C73A38"),
    ("firehose", "L-61E2CA1D"),
    ("firehose", "L-7681C790"),
    ("firehose", "L-937C879B"),
    ("firehose", "L-CC1FDDAA"),
    ("firehose", "L-ED7DA7FC"),
    ("firehose", "L-FDED3C5B"),
    ("fms", "L-0745D646"),
    ("fms", "L-07FCB28B"),
    ("fms", "L-0B28E140"),
    ("fms", "L-0E131699"),
    ("fms", "L-1608788B"),
    ("fms", "L-1E778CA5"),
    ("fms", "L-25F99602"),
    ("fms", "L-3BC4947E"),
    ("fms", "L-3DCEAE01"),
    ("fms", "L-3F3557EB"),
    ("fms", "L-450C679C"),
    ("fms", "L-49265FF2"),
    ("fms", "L-4E3D82C8"),
    ("fms", "L-573ADF04"),
    ("fms", "L-5AB0AC42"),
    ("fms", "L-72AC804F"),
    ("fms", "L-742188BD"),
    ("fms", "L-743D9B7E"),
    ("fms", "L-74591874"),
    ("fms", "L-7AB3A351"),
    ("fms", "L-7F63BF51"),
    ("fms", "L-941CF907"),
    ("fms", "L-98694ACE"),
    ("fms", "L-9E030C62"),
    ("fms", "L-BCA8AD34"),
    ("fms", "L-C6CF2DBB"),
    ("fms", "L-C80E7B9E"),
    ("fms", "L-CA0A307F"),
    ("fms", "L-DE1D154D"),
    ("fms", "L-F4032167"),
    ("fsx", "L-0CD18A5D"),
    ("fsx", "L-15D9FE87"),
    ("fsx", "L-4E6C2FB3"),
    ("fsx", "L-4EDE4065"),
    ("fsx", "L-57578687"),
    ("fsx", "L-59D0763F"),
    ("fsx", "L-5B89F9CE"),
    ("fsx", "L-60836D3E"),
    ("fsx", "L-736F3D6F"),
    ("fsx", "L-7D5FDD38"),
    ("fsx", "L-84EAF187"),
    ("fsx", "L-88479C21"),
    ("fsx", "L-8F1B9C74"),
    ("fsx", "L-901C77F5"),
    ("fsx", "L-9AFA1F09"),
    ("fsx", "L-AD2FC696"),
    ("fsx", "L-B7391FCE"),
    ("fsx", "L-C28C1403"),
    ("fsx", "L-C431DBA3"),
    ("fsx", "L-C48231E5"),
    ("fsx", "L-C5F860DD"),
    ("fsx", "L-C8640C82"),
    ("fsx", "L-CD5E0524"),
    ("fsx", "L-DD0F7417"),
    ("fsx", "L-E24B4DE4"),
    ("fsx", "L-E2C89679"),
    ("fsx", "L-E43BDB2E"),
    ("fsx", "L-E94C1C19"),
    ("fsx", "L-FD6F2F22"),
    ("fsx", "L-FD89CA8A"),
    ("gamelift", "L-0536A98D"),
    ("gamelift", "L-11948650"),
    ("gamelift", "L-22451070"),
    ("gamelift", "L-24BE0A39"),
    ("gamelift", "L-293B0017"),
    ("gamelift", "L-3A43EF3C"),
    ("gamelift", "L-51AF299A"),
    ("gamelift", "L-55650DB7"),
    ("gamelift", "L-593688D9"),
    ("gamelift", "L-67477D57"),
    ("gamelift", "L-73F6E300"),
    ("gamelift", "L-8AE49BBD"),
    ("gamelift", "L-8D885299"),
    ("gamelift", "L-8EBC4E87"),
    ("gamelift", "L-90D24F1B"),
    ("gamelift", "L-9F9DE0B2"),
    ("gamelift", "L-A8C1B434"),
    ("gamelift", "L-AED4A06A"),
    ("gamelift", "L-BB62CF1D"),
    ("gamelift", "L-BFEEB817"),
    ("gamelift", "L-C30AA854"),
    ("gamelift", "L-C6F4238C"),
    ("gamelift", "L-CCBBB4DA"),
    ("gamelift", "L-E8D8BD94"),
    ("gamelift", "L-EC03D793"),
    ("gamelift", "L-ED58E8F8"),
    ("gamelift", "L-FDDD1260"),
    ("glacier", "L-0D9D2530"),
    ("glacier", "L-153F5D45"),
    ("glacier", "L-4BF4C362"),
    ("glacier", "L-982DEA76"),
    ("glacier", "L-D1C67346"),
    ("glacier", "L-E0D19AD1"),
    ("glacier", "L-E96017CA"),
    ("glacier", "L-F127DF29"),
    ("glue", "L-04CEE988"),
    ("glue", "L-071ABE08"),
    ("glue", "L-08F3B322"),
    ("glue", "L-096DBB95"),
    ("glue", "L-11FA2C1A"),
    ("glue", "L-1DD415D5"),
    ("glue", "L-2C3F5401"),
    ("glue", "L-2E9FB93F"),
    ("glue", "L-337244C9"),
    ("glue", "L-3890A802"),
    ("glue", "L-4071B0E3"),
    ("glue", "L-4256D6D2"),
    ("glue", "L-4F4B522D"),
    ("glue", "L-5E4153CA"),
    ("glue", "L-611FDDE4"),
    ("glue", "L-64A88F0A"),
    ("glue", "L-76EC689B"),
    ("glue", "L-785C1F39"),
    ("glue", "L-7DD7C33A"),
    ("glue", "L-83192DBF"),
    ("glue", "L-83A59AA6"),
    ("glue", "L-87F5991C"),
    ("glue", "L-94D025B7"),
    ("glue", "L-958728D1"),
    ("glue", "L-A457CBAB"),
    ("glue", "L-A57B5BCE"),
    ("glue", "L-ACAC12B6"),
    ("glue", "L-AD871090"),
    ("glue", "L-B4840D4D"),
    ("glue", "L-B78896B9"),
    ("glue", "L-B8497671"),
    ("glue", "L-C3952F6C"),
    ("glue", "L-CB69DFA0"),
    ("glue", "L-D90AC94B"),
    ("glue", "L-D987EC31"),
    ("glue", "L-DBA56E2F"),
    ("glue", "L-E15CE20A"),
    ("glue", "L-E6C807E7"),
    ("glue", "L-EAC30D24"),
    ("glue", "L-EEC98450"),
    ("glue", "L-F1653A6D"),
    ("glue", "L-F1AAB534"),
    ("glue", "L-F545C262"),
    ("glue", "L-F574AED9"),
    ("glue", "L-F953935E"),
    ("glue", "L-FB210BD9"),
    ("glue", "L-FEBBFA7A"),
    ("groundstation", "L-09DEC198"),
    ("groundstation", "L-5342B9BF"),
    ("groundstation", "L-5CCF0BC2"),
    ("groundstation", "L-98A63A85"),
    ("groundstation", "L-BD84767C"),
    ("groundstation", "L-CCFDE387"),
    ("groundstation", "L-D6A1915B"),
    ("groundstation", "L-DE376FC5"),
    ("groundstation", "L-DF7B6DEC"),
    ("groundstation", "L-FED20749"),
    ("guardduty", "L-2C0E14B9"),
    ("guardduty", "L-373487E5"),
    ("guardduty", "L-8B0D7135"),
    ("guardduty", "L-9ABF7A23"),
    ("guardduty", "L-AFBA2260"),
    ("guardduty", "L-F51A2830"),
    ("guardduty", "L-FA6D6E3D"),
    ("imagebuilder", "L-0DF2752F"),
    ("imagebuilder", "L-10D22E0D"),
    ("imagebuilder", "L-1DF98342"),
    ("imagebuilder", "L-22EAF8F8"),
    ("imagebuilder", "L-28A502FD"),
    ("imagebuilder", "L-2BAA05D8"),
    ("imagebuilder", "L-31E5726E"),
    ("imagebuilder", "L-59EC76F6"),
    ("imagebuilder", "L-5E848392"),
    ("imagebuilder", "L-6105A4EE"),
    ("imagebuilder", "L-69312229"),
    ("imagebuilder", "L-7A0E01E3"),
    ("imagebuilder", "L-84F5AB81"),
    ("imagebuilder", "L-9B183655"),
    ("imagebuilder", "L-A26B27A7"),
    ("imagebuilder", "L-A58CFBED"),
    ("imagebuilder", "L-B588F8CA"),
    ("imagebuilder", "L-BA4D191B"),
    ("imagebuilder", "L-D5665818"),
    ("imagebuilder", "L-D5DC1FB3"),
    ("imagebuilder", "L-D9E1BB3C"),
    ("imagebuilder", "L-D9EF98D9"),
    ("imagebuilder", "L-E49825CC"),
    ("kafka", "L-36D29E8C"),
    ("kafka", "L-8F940D28"),
    ("kafka", "L-B2FDE22A"),
    ("kafka", "L-FAB9E493"),
    ("kinesis", "L-0918CF54"),
    ("kms", "L-275D92F3"),
    ("kms", "L-8A08DAEA"),
    ("lambda", "L-25EEC0A4"),
    ("lambda", "L-2ACBD22F"),
    ("lambda", "L-4273958C"),
    ("lambda", "L-72E0D058"),
    ("lambda", "L-B2AA0F47"),
    ("lambda", "L-B430C318"),
    ("lambda", "L-B82A30EA"),
    ("lambda", "L-B99A9384"),
    ("lambda", "L-F55076EC"),
    ("monitoring", "L-6FCAAA2E"),
    ("ram", "L-2870BE9D"),
    ("ram", "L-4A6CEE66"),
    ("ram", "L-595828F9"),
    ("ram", "L-8491BF81"),
    ("rds", "L-9372BAB3"),
    ("rds", "L-DE55804A"),
    ("rds", "L-E3721453"),
    ("redshift", "L-4E1FE923"),
    ("redshift", "L-8AFA7F04"),
    ("redshift", "L-99F53FC8"),
    ("redshift", "L-BB966966"),
    ("redshift-serverless", "L-1854027F"),
    ("redshift-serverless", "L-51089E7F"),
    ("redshift-serverless", "L-96772D74"),
    ("route53resolver", "L-02CC8B74"),
    ("route53resolver", "L-15219E1D"),
    ("route53resolver", "L-1B2BDF0A"),
    ("route53resolver", "L-51D8A1FB"),
    ("route53resolver", "L-D74B6237"),
    ("s3", "L-1A9B467F"),
    ("s3", "L-349AD9CA"),
    ("s3", "L-89BABEE8"),
    ("s3", "L-B461D596"),
    ("s3", "L-DC2B2D3D"),
    ("scheduler", "L-66629D52"),
    ("scheduler", "L-6F4B17DE"),
    ("scheduler", "L-EE5D6FF0"),
    ("scheduler", "L-FB03E819"),
    ("secretsmanager", "L-2D877201"),
    ("secretsmanager", "L-385CE7B7"),
    ("secretsmanager", "L-8D82A813"),
    ("secretsmanager", "L-CBA4CAC4"),
    ("ses", "L-804C8AE8"),
    ("ses", "L-CDEF9B6B"),
    ("signin", "L-42359A23"),
    ("vpc", "L-045E6E9C"),
    ("vpc", "L-085A6257"),
    ("vpc", "L-0EA8095F"),
    ("vpc", "L-1B52E74A"),
    ("vpc", "L-29B6F2EB"),
    ("vpc", "L-2AEEBF1A"),
    ("vpc", "L-2AFB9258"),
    ("vpc", "L-407747CB"),
    ("vpc", "L-45FE3B85"),
    ("vpc", "L-589F43AA"),
    ("vpc", "L-7E9ECCDB"),
    ("vpc", "L-93826ACB"),
    ("vpc", "L-A4707A72"),
    ("vpc", "L-B4A6D682"),
    ("vpc", "L-F678F1CE"),
    ("vpc", "L-FE5A380F"),
];

/// The `(ServiceCode, QuotaCode)` pairs this catalog cites from a SERVICE'S OWN API
/// rather than from AWS's minted namespace: `b2bi` reports its profile limit under
/// `ProfilesPerAccount` in its own `ServiceQuotaExceededException`, so a client that
/// reads that refusal finds the quota here under the code the refusal named.
const CITED_SERVICE_QUOTA_CODES: &[(&str, &str)] = &[("b2bi", "ProfilesPerAccount")];

/// The service codes AWS's inventory lists for which this catalog carries no display
/// name it can cite, and which therefore report their code (see the tail of
/// `src/state.rs::SEED_SERVICE_CATALOG`). Every OTHER code reports a display name.
const CODES_WITHOUT_A_CITED_DISPLAY_NAME: &[&str] = &[
    "eks-mcp",
    "elemental-inference",
    "health-agent",
    "interconnect",
    "networkinsights",
    "s3files",
    "securityagent",
    "supportauthz",
    "ts",
];

/// Whether `code` has the shape of a code AWS MINTS: `L-` and eight upper-case hex
/// characters, the form every `QuotaCode` in AWS's published catalog has.
///
/// `pub(crate)` so another suite asks this question through the one copy of it:
/// `default_quota_published_codes.rs` asserts the same namespace rule for the services
/// whose whole published catalog sits in it, so the two suites cannot disagree about
/// what the namespace IS.
pub(crate) fn is_aws_minted_shape(code: &str) -> bool {
    code.strip_prefix("L-").is_some_and(|rest| {
        rest.len() == 8
            && rest
                .chars()
                .all(|char| char.is_ascii_digit() || ('A'..='F').contains(&char))
    })
}

/// The descriptive code a quota name yields: its words, each capitalised, joined,
/// with a `Quota` prefix where that would not start with a letter and truncated to
/// the modelled maximum. The rule
/// `src/state.rs::descriptive_quota_code` implements, restated in full here so the
/// test fails if the implementation drifts from it.
fn descriptive_code(quota_name: &str) -> String {
    let mut code = String::new();
    for word in quota_name.split(|char: char| !char.is_ascii_alphanumeric()) {
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            code.push(first.to_ascii_uppercase());
            code.extend(chars);
        }
    }
    if !code.starts_with(|char: char| char.is_ascii_alphabetic()) {
        code.insert_str(0, "Quota");
    }
    if code.len() < 2 {
        code.push_str("Quota");
    }
    code.truncate(128);
    code
}

/// The rule itself, over names that reach every branch of it — including the two no
/// catalog row trips today, so the restated rule above is complete rather than
/// agreeing with the implementation by luck.
#[test]
fn a_descriptive_code_is_the_quota_name_as_one_modelled_identifier() {
    // Rows: a plain name, punctuation and digits inside words, a name that starts
    // with a digit, a name with a single letter in it, and a name far longer than
    // the modelled 128-character maximum.
    for name in [
        "Workgroups per Region",
        "Alternate domain names (CNAMEs) per distribution",
        "Running On-Demand Standard (A, C, D, H, I, M, R, T, Z) instances",
        "concurrent-evaluations-per-agent-space",
        "5xx responses per minute",
        "A",
        "!!",
        &"Extremely long quota name that AWS would never publish ".repeat(8),
    ] {
        let code = descriptive_quota_code(name);
        assert_eq!(
            code,
            descriptive_code(name),
            "case {name:?}: expected the descriptive code of the name, got {code:?}"
        );
        assert!(
            (2..=128).contains(&code.len())
                && code.starts_with(|char: char| char.is_ascii_alphabetic())
                && code
                    .chars()
                    .all(|char| char.is_ascii_alphanumeric() || char == '-'),
            "case {name:?}: every descriptive code satisfies the modelled QuotaCode \
             pattern, got {code:?}"
        );
    }
}

/// A code in AWS's minted namespace is published only where this catalog cites it;
/// every other quota is published under the descriptive code of its own name, and
/// every published code satisfies the modelled `QuotaCode` pattern
/// (`smithy_query(query="shape", shape="ServiceQuota")`: length 1..128, pattern
/// `^[a-zA-Z][a-zA-Z0-9-]{1,128}$`).
#[tokio::test]
async fn only_a_cited_quota_code_sits_in_the_aws_minted_namespace() {
    let engine = emulator();
    let client = client(&engine, "us-west-2");

    let cited: BTreeSet<(&str, &str)> = CITED_AWS_QUOTA_CODES.iter().copied().collect();
    let catalog = whole_catalog(&client).await;
    let mut minted = 0_usize;
    let mut descriptive = 0_usize;

    for (service_code, quotas) in &catalog {
        for quota in quotas {
            let code = quota["QuotaCode"]
                .as_str()
                .unwrap_or_else(|| panic!("case {service_code}: a quota without a code: {quota}"));
            let name = quota["QuotaName"]
                .as_str()
                .unwrap_or_else(|| panic!("case {service_code}: a quota without a name: {quota}"));
            let case = format!("{service_code}/{code}");

            // The modelled constraints, for every row.
            assert!(
                (2..=128).contains(&code.len())
                    && code.starts_with(|char: char| char.is_ascii_alphabetic())
                    && code
                        .chars()
                        .all(|char| char.is_ascii_alphanumeric() || char == '-'),
                "case {case}: expected a code satisfying the modelled QuotaCode pattern, got \
                 {code:?}"
            );

            if CITED_SERVICE_QUOTA_CODES.contains(&(service_code.as_str(), code)) {
                // A code the service's own API publishes: cited, so neither minted
                // nor descriptive.
            } else if is_aws_minted_shape(code) {
                assert!(
                    cited.contains(&(service_code.as_str(), code)),
                    "case {case}: a code in AWS's minted L- namespace must be one this catalog \
                     cites from AWS, got the unattributable {code:?} for {name:?}"
                );
                minted += 1;
            } else {
                assert_eq!(
                    code,
                    descriptive_quota_code(name),
                    "case {case}: a quota whose AWS code this catalog does not cite is published \
                     under its name's descriptive code, expected {:?}, got {code:?}",
                    descriptive_quota_code(name)
                );
                descriptive += 1;
            }
        }
    }

    // Both branches of the rule are exercised by the table above.
    assert!(
        minted >= CITED_AWS_QUOTA_CODES.len() && descriptive > 100,
        "case catalog: expected both cited and descriptive codes to be published, got \
         {minted} cited and {descriptive} descriptive"
    );

    // The pairs this catalog cites are published, each under the service that cites
    // it — a cited code left unpublished would make this suite vacuous.
    for (service_code, quota_code) in CITED_AWS_QUOTA_CODES {
        let published = catalog
            .get(*service_code)
            .unwrap_or_else(|| panic!("case {service_code}: expected the service to be listed"));
        assert!(
            published
                .iter()
                .any(|quota| quota["QuotaCode"] == json!(quota_code)),
            "case {service_code}/{quota_code}: expected the cited quota to be published, got {} \
             quotas",
            published.len()
        );
    }
}

/// The published rows of the services whose default sets this suite pins, each read
/// back through `GetAWSDefaultServiceQuota`.
///
/// SOURCE: AWS's published default-quota data for each `ServiceCode` (what
/// `aws service-quotas list-aws-default-service-quotas --service-code <code>`
/// reports). Rows cover a countable quota, a not-adjustable one, a unit other than
/// `None`, and a rate quota with a `Period`.
#[tokio::test]
async fn a_cited_default_quota_reads_back_with_the_values_aws_publishes() {
    let engine = emulator();
    let client = client(&engine, "us-west-2");

    struct Row {
        service_code: &'static str,
        quota_code: &'static str,
        quota_name: &'static str,
        value: f64,
        unit: &'static str,
        adjustable: bool,
        period: Option<(i32, &'static str)>,
    }

    let rows = [
        Row {
            service_code: "access-analyzer",
            quota_code: "L-96F55078",
            quota_name: "Policy generation CloudTrail time range",
            value: 90.0,
            unit: "None",
            adjustable: false,
            period: None,
        },
        Row {
            service_code: "access-analyzer",
            quota_code: "L-1E51937C",
            quota_name: "Archive rules per analyzer",
            value: 100.0,
            unit: "None",
            adjustable: true,
            period: None,
        },
        Row {
            service_code: "account-access",
            quota_code: "L-42A1D874",
            quota_name: "Group entitlements per role",
            value: 20.0,
            unit: "None",
            adjustable: true,
            period: None,
        },
        Row {
            service_code: "agent-registry",
            quota_code: "L-7F384C37",
            quota_name: "InvokeRegistryMcp requests per second",
            value: 10.0,
            unit: "Count/Second",
            adjustable: true,
            period: Some((1, "SECOND")),
        },
        Row {
            service_code: "aidevops",
            quota_code: "L-4FF187E6",
            quota_name: "concurrent-evaluations-per-agent-space",
            value: 1.0,
            unit: "Count",
            adjustable: false,
            period: None,
        },
        Row {
            service_code: "airflow",
            quota_code: "L-52DCDA32",
            quota_name: "Web servers per environment",
            value: 5.0,
            unit: "None",
            adjustable: true,
            period: None,
        },
        Row {
            service_code: "acm-pca",
            quota_code: "L-01B8608D",
            quota_name: "Rate of CreateCertificateAuthority requests",
            value: 1.0,
            unit: "None",
            adjustable: false,
            period: Some((1, "SECOND")),
        },
        // The quota whose usage AWS publishes as a RESOURCE count: its period is the
        // window that metric is published over, which the row does not state
        // separately.
        Row {
            service_code: "aiops",
            quota_code: "L-EF31E0C4",
            quota_name: "Concurrent active investigations",
            value: 2.0,
            unit: "None",
            adjustable: false,
            period: Some((5, "MINUTE")),
        },
        // A quota the issue behind this suite never cited, carried by the catalog
        // from AWS's published VPC data.
        Row {
            service_code: "vpc",
            quota_code: "L-085A6257",
            quota_name: "IPv6 CIDR blocks per VPC",
            value: 1.0,
            unit: "None",
            adjustable: false,
            period: None,
        },
    ];

    for row in &rows {
        let case = format!("{}/{}", row.service_code, row.quota_code);
        let pair = json!({"ServiceCode": row.service_code, "QuotaCode": row.quota_code});
        let quota = client.ok("GetAWSDefaultServiceQuota", pair.clone()).await["Quota"].clone();

        assert_eq!(
            quota["QuotaName"],
            json!(row.quota_name),
            "case {case}: expected the published quota name, got {quota}"
        );
        assert_eq!(
            quota["Value"],
            json!(row.value),
            "case {case}: expected the published default value, got {quota}"
        );
        assert_eq!(
            quota["Unit"],
            json!(row.unit),
            "case {case}: expected the published unit, got {quota}"
        );
        assert_eq!(
            quota["Adjustable"],
            json!(row.adjustable),
            "case {case}: expected the published adjustability, got {quota}"
        );
        match row.period {
            Some((value, unit)) => assert_eq!(
                quota["Period"],
                json!({"PeriodValue": value, "PeriodUnit": unit}),
                "case {case}: a rate quota reports the window AWS publishes it over, got {quota}"
            ),
            None => assert_eq!(
                quota["Period"],
                Value::Null,
                "case {case}: a quota AWS publishes no rate window for reports none, got {quota}"
            ),
        }

        // The listing reports the same row the single read does.
        let listed = defaults(&client, row.service_code).await;
        assert!(
            listed.contains(&quota),
            "case {case}: the listed entry must be the one the single read returns, got {listed:?}"
        );
    }
}

/// A `ServiceName` is a display name, never the `ServiceCode` echoed back — for
/// every service the inventory lists except the codes this catalog cannot cite a
/// name for.
#[tokio::test]
async fn a_listed_service_reports_a_display_name_not_its_own_code() {
    let engine = emulator();
    let client = client(&engine, "eu-west-1");

    let listed = inventory(&client).await;
    let names: BTreeMap<String, String> = listed
        .iter()
        .map(|service| {
            (
                service["ServiceCode"].as_str().unwrap_or_default().to_string(),
                service["ServiceName"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();

    let echoing: BTreeSet<&str> = names
        .iter()
        .filter(|(code, name)| code == name)
        .map(|(code, _)| code.as_str())
        .collect();
    let expected: BTreeSet<&str> = CODES_WITHOUT_A_CITED_DISPLAY_NAME.iter().copied().collect();
    assert_eq!(
        echoing, expected,
        "case ListServices: only a code with no display name this catalog can cite may report \
         its own code as its name"
    );

    // The display names, as a table, with whether the catalog carries published
    // default quotas for the code. The last three rows are codes the issue behind
    // this suite never cited.
    for (code, name, carries_quotas) in [
        ("access-analyzer", "Access Analyzer", true),
        ("account-access", "Account access manager", true),
        ("aco-automation", "AWS Compute Optimizer Automation", true),
        ("agent-registry", "AWS Agent Registry", true),
        ("aidevops", "AWS DevOps Agent", true),
        ("aiops", "Amazon AI Operations", true),
        (
            "app-integrations",
            "Amazon Connect Application Integrations",
            false,
        ),
        ("aps", "Amazon Managed Prometheus", false),
        ("robomaker", "AWS RoboMaker", false),
        ("security-ir", "AWS Security Incident Response", false),
        ("ssm-sap", "AWS Systems Manager for SAP", false),
    ] {
        assert_eq!(
            names.get(code).map(String::as_str),
            Some(name),
            "case {code}: expected the display name AWS lists the code under, got {:?}",
            names.get(code)
        );
        // And a code the inventory lists resolves its defaults, so a client that
        // discovered it here can read its quotas: the rows the catalog carries
        // quotas for come back with entries, and a code whose published defaults it
        // does not carry comes back with the empty list its own code earns it.
        let quotas = client
            .ok(
                "ListAWSDefaultServiceQuotas",
                json!({"ServiceCode": code}),
            )
            .await;
        let listed = quotas["Quotas"]
            .as_array()
            .unwrap_or_else(|| panic!("case {code}: expected a Quotas list, got {quotas}"));
        assert_eq!(
            !listed.is_empty(),
            carries_quotas,
            "case {code}: expected the catalog to carry {}quotas for this code, got {} entries",
            if carries_quotas { "" } else { "no " },
            listed.len()
        );
    }
}

/// A lookup by a code in AWS's minted namespace that this catalog does not cite
/// resolves NOTHING — which is what keeps a client that holds a real AWS quota code
/// from resolving it against a quota this emulator invented — while the descriptive
/// code a quota IS published under resolves that quota.
#[tokio::test]
async fn a_minted_code_this_catalog_does_not_cite_resolves_nothing() {
    let engine = emulator();
    let client = client(&engine, "ap-northeast-1");

    // Rows: AWS-minted codes this catalog does not cite — one AWS publishes for the
    // service asked for, one it publishes for a DIFFERENT service, and one that is
    // merely well formed. A code of this shape used to be synthesized from the quota
    // name, which is how a lookup like the first row used to succeed.
    for (service_code, quota_code, why) in [
        ("airflow", "L-CFEFCF95", "a code no quota of this service is published under"),
        ("access-analyzer", "L-4599575C", "an AWS code this catalog does not carry"),
        ("monitoring", "L-1216C47A", "an EC2 code under CloudWatch"),
        ("athena", "L-00000000", "a well-formed code nothing publishes"),
    ] {
        let case = format!("{service_code}/{quota_code}");
        for operation in ["GetAWSDefaultServiceQuota", "GetServiceQuota"] {
            let (status, error) = client
                .refused(
                    operation,
                    json!({"ServiceCode": service_code, "QuotaCode": quota_code}),
                )
                .await;
            assert_eq!(
                (status, error.as_str()),
                (400, "NoSuchResourceException"),
                "case {case} ({why}): {operation} must refuse a quota it does not publish"
            );
        }
    }

    // Rows: the descriptive code of a published quota, for services whose rows carry
    // no AWS code — each resolves the quota whose name it was built from.
    for (service_code, quota_code, quota_name) in [
        ("amplify", "AppsPerRegion", "Apps per Region"),
        ("iot", "RulesPerAccount", "Rules per account"),
        ("vpc", "PrefixListsPerRegion", "Prefix lists per Region"),
        // The one row whose code the SERVICE's own refusal publishes.
        ("b2bi", "ProfilesPerAccount", "Customer profiles per account"),
    ] {
        let case = format!("{service_code}/{quota_code}");
        let quota = client
            .ok(
                "GetAWSDefaultServiceQuota",
                json!({"ServiceCode": service_code, "QuotaCode": quota_code}),
            )
            .await["Quota"]
            .clone();
        assert_eq!(
            quota["QuotaName"],
            json!(quota_name),
            "case {case}: the code must resolve the quota it names, got {quota}"
        );
    }
}
