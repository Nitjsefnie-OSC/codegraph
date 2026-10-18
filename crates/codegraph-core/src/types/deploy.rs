use serde::{Deserialize, Serialize};

/// The graph-native mirror of one compiled rexlang `.deploy` deployment
/// artifact (`rex_ir::deploy::DeployModel`, rexlang #49): the declared
/// applications (components + connections), profiles (target + defaults +
/// policies + mappings), and deployments. codegraph-core takes no rex-*
/// deps, so component kinds and targets cross this boundary as their
/// authored keywords (`"api"`, `"cloudflareWorkers"`, ...) and settings as
/// path/value pairs with a tagged value enum (the `EvtEventField::type_json`
/// payload precedent, minus the rex types — deploy values are plain).
///
/// Deployments are file-scoped: `deployment ... for <application>` and
/// `use <profile>` are intra-file names kept verbatim (resolved by
/// consumers against the same model — resolution is never a graph-plane
/// concern). Everything is as-authored; effective settings are a consumer
/// computation (rex-ir never serializes the merge).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeployModelGraph {
    /// The `.deploy` source file the model was compiled from. Groups the
    /// models on read: one `DeployModelGraph` per source path.
    pub source_path: String,
    /// Declared applications, in declaration order.
    pub applications: Vec<DeployApplicationNode>,
    /// Declared profiles, in declaration order.
    pub profiles: Vec<DeployProfileNode>,
    /// Declared deployments, in declaration order.
    pub deployments: Vec<DeployDeploymentNode>,
}

/// A declared application: the logical architecture (rex-ir
/// `DeployApplication` plus its source path).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeployApplicationNode {
    /// The `.deploy` source file the application was declared in.
    pub source_path: String,
    /// Application name, unique within its contract file.
    pub name: String,
    /// Components in declaration order.
    pub components: Vec<DeployComponentNode>,
    /// `connects a -> b` edges in declaration order; endpoints are
    /// component names of this application.
    pub connections: Vec<DeployConnectionNode>,
}

/// A declared component: one runtime piece of an application (rex-ir
/// `DeployComponent`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeployComponentNode {
    /// The owning application name.
    pub application: String,
    /// Component name, unique within its application.
    pub name: String,
    /// The component kind keyword: `"api" | "worker" | "database" |
    /// "queue" | "objectStore" | "frontend"`.
    pub kind: String,
    /// Declared `requires` capability names, as authored (the closed
    /// vocabulary is a rex-driver validation concern, not carried here).
    pub requires: Vec<String>,
    /// Baseline settings in declaration order.
    pub settings: Vec<DeploySettingNode>,
    /// `design "<path>[#<Module>]"` bindings in declaration order; the
    /// source is the import path exactly as written.
    pub designs: Vec<DeployBindingNode>,
    /// `flow "<path>[#<Module>]"` bindings in declaration order.
    pub flows: Vec<DeployBindingNode>,
    /// Declaration order within the application.
    pub ordinal: usize,
}

/// A `connects from -> to` edge (rex-ir `DeployConnection`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeployConnectionNode {
    /// The owning application name.
    pub application: String,
    /// The calling component.
    pub from: String,
    /// The called component.
    pub to: String,
}

/// One setting: a dotted path and its value (rex-ir `DeploySetting`).
/// Paths stay exactly as authored: single-segment in component baselines,
/// `component.setting` two-segment in profile defaults and deployment
/// configure blocks (splitting/scoping is a consumer concern).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeploySettingNode {
    /// Setting path exactly as written (e.g. `"runtime"`,
    /// `"api.replicas"`).
    pub path: String,
    /// The setting value.
    pub value: DeployValueNode,
}

/// A tagged setting value (rex-ir `DeployValue`): an integer, a quoted
/// text, or a bare word.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum DeployValueNode {
    /// An integer literal.
    Int(i64),
    /// A quoted text literal.
    Text(String),
    /// A bare word literal.
    Word(String),
}

/// A `design`/`flow` binding (rex-ir `DesignBinding`/`FlowBinding`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeployBindingNode {
    /// The import path exactly as written.
    pub source: String,
    /// The `#<Module>` suffix, when the binding carried one.
    pub module: Option<String>,
}

/// A declared profile: one target plus its defaults, policies, and
/// resource mappings (rex-ir `DeployProfile`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeployProfileNode {
    /// The `.deploy` source file the profile was declared in.
    pub source_path: String,
    /// Profile name, unique within its contract file.
    pub name: String,
    /// The target keyword: `"standalone" | "dockerCompose" | "kubernetes" |
    /// "cloudflareWorkers"`.
    pub target: String,
    /// `defaults { component.setting: value }` lines in declaration order.
    pub defaults: Vec<DeploySettingNode>,
    /// `require (...)`/`prohibit (...)` policies in declaration order.
    pub policies: Vec<DeployPolicyNode>,
    /// `mappings { kind -> platform.resource }` lines in declaration
    /// order.
    pub mappings: Vec<DeployMappingNode>,
}

/// A `require (...)`/`prohibit (...)` policy (rex-ir `DeployPolicy`). The
/// expression is the verbatim source text between the parentheses — the
/// IR never parses or reformats it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeployPolicyNode {
    /// The owning profile name.
    pub profile: String,
    /// `true` for `prohibit (...)`, `false` for `require (...)`.
    pub prohibit: bool,
    /// The verbatim rex-expr source text.
    pub expression: String,
}

/// A `kind -> platform.resource` mapping line (rex-ir `DeployMapping`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeployMappingNode {
    /// The owning profile name.
    pub profile: String,
    /// The mapped component-kind keyword.
    pub kind: String,
    /// The platform resource id, as authored (e.g. `cloudflare.worker`,
    /// `cloudflare.hyperdrive`).
    pub resource: String,
}

/// A declared deployment: one application projected through one profile
/// (rex-ir `DeployDeployment`). All cross-references are intra-file names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeployDeploymentNode {
    /// The `.deploy` source file the deployment was declared in.
    pub source_path: String,
    /// Deployment name, unique within its contract file.
    pub name: String,
    /// The deployed application's name.
    pub application: String,
    /// The selected profile's name.
    pub profile: String,
    /// `configure { component.setting: value }` overrides in declaration
    /// order.
    pub configure: Vec<DeploySettingNode>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_node_round_trips_through_json() {
        let value = DeployValueNode::Int(4);
        let json = serde_json::to_string(&value).unwrap();
        assert_eq!(json, r#"{"type":"int","value":4}"#);
        assert_eq!(
            serde_json::from_str::<DeployValueNode>(&json).unwrap(),
            value
        );
    }

    #[test]
    fn empty_model_has_no_applications() {
        let model = DeployModelGraph {
            source_path: "svc.deploy".to_string(),
            applications: Vec::new(),
            profiles: Vec::new(),
            deployments: Vec::new(),
        };
        assert!(model.applications.is_empty());
    }
}
