# AGENTS.md

## Project structure

Workspace root `Cargo.toml` with 12 crates:

| Crate | Purpose |
|-------|---------|
| `codegraph` | Main binary: CLI, ingest, classify, validate + project init/doctor/add-domain lifecycle |
| `codegraph-generate` | The 60+ generators, generator traits, template engine, profile/BuildPlan |
| `codegraph-core` | Graph data model: `GraphQuerier`, `GraphIngestor`, node/edge types |
| `codegraph-grafeo` | Grafeo graph database adapter implementing core traits |
| `codegraph-backend` | Backend factory (currently Grafeo-only) |
| `codegraph-type-contracts` | Type system: PgType, RustType, DddFieldProjection |
| `codegraph-naming` | Identifier naming: snake_case, PascalCase, PG identifier handling |
| `codegraph-classifier` | Config-driven JSON schema type classification |
| `codegraph-config` | Domain config parsing (`domains.toml`, classifier.toml, profiles.toml) + `OpsManifest` |
| `codegraph-ext-points` | Extension points config types |
| `codegraph-workflow` | Generic state machine workflow engine (SeaORM) |
| `ast-ifml` | auto-lsp AST definitions for IFML |
| `codegraph-ops` | Rust test & deploy harness (see "Ops Harness" section) |

The 12 members above are the workspace (the IFML DSL parser lives upstream in
the rexlang repo as `rex-ifml`, consumed as a rev-pinned git dep — see the
"IFML Integration" section). The tree also carries
non-workspace directories: `codegraph-vscode/` (IFML VS Code extension),
`crates/tree-sitter-ifml/` and `crates/tree-sitter-mox/` (editor grammars),
and `crates/review/` (regenerated fixture app exercised by
`grafeo_e2e_tests`). None are workspace members.

## mox-First Modeling (epic #228)

`.mox` (rexlang) is the **primary text-based modeling language**; JSON
Schema is the legacy/import surface. `codegraph run --mox-files model.mox
--config domains.toml --output out/` works without `--schemas`. Pipeline
order in `driver::run`: **Pass 1 mox** (bridges classes → Schema/Property/
CodeList nodes; provenance `custom_annotations["source"]="mox"`; entity vs
VO is author-declarative — a class targeted only by `contains` is a VO,
`refers` wins) → **Pass 1a schemas** (skips mox-covered titles; mox wins
title conflicts by ordering) → IFML/api/openapi → auto-classify (bypasses
mox schemas). `--schemas` alone emits a deprecation WARN pointing at
`codegraph migrate` (which converts JSON Schema dirs to `.mox` packages,
parse-verified). Key files: `crates/codegraph/src/ingest/mox_ingest.rs`
(bridge + import scan + `wire_alias_refs`), `ingest/async_ingest.rs`
(`ingest_schemas_with_skips`, `ingest_imported_schemas`),
`classify/mod.rs` (priority-0 mox bypass), `migrate.rs` + embedded
`migrate/stdlib.mox`. Equivalence gate:
`crates/codegraph/tests/mox_equivalence_tests.rs` pins full-tree
byte-identity between equivalent JSON Schema and `.mox` models.
`docs/mox-simplification-ledger.md` is the deletion roadmap for the
JSON-Schema inference machinery.

### `import schema` (JSON Schema types from .mox)

```mox
package todo

import schema "schemas/todo_item.json" as TodoItem

class TodoListType {
    refers TodoItem[] items
}
```

rexlang owns the NAME (opaque nominal registration, `SchemaImports`
provider API, upstream `yestechgroup/rexlang` rev `699b3a5`+); codegraph
owns the GRAPH: imported files flow through the existing classifier-aware
JSON pipeline (`ingest_imported_schemas` — wrapper/range/codelist config
keeps working), and `wire_alias_refs` resolves deferred alias-typed
features to real titles after the schema pass (exact → `+type_suffix` →
warning). Missing/invalid import targets are **hard errors** (structural,
unlike advisory `.actor` imports); `doctor --mox-files` validates them.
Import scan is a line-scan (declaration must start its trimmed line with
`import schema `). Actor-policy domains carrying imports compile via
`compile_actors_str_with_imports`.

### LSP for `.mox` (MoxState)

`codegraph lsp --mox-files <file>...` builds an in-process `MoxState`
(`crates/codegraph/src/lsp/mox.rs`) from the compiled mox workspace and
serves: unknown-type diagnostics (tree-sitter queries over `type:`/
`superclass:` fields), import-target-must-exist errors, completions in
`refers`/`contains`/`container`/`extends`/`on` contexts and type
positions, and type-position hover. Degradation contract (pinned by
tests): without `--mox-files`, `.mox` documents are quiet on semantics;
parser-level syntax diagnostics still flow; the LSP never fails to start.
Editor grammar: `codegraph-vscode/grammar-mox/` (corpus-tested) →
committed parser in `crates/tree-sitter-mox/` (regenerate with
`npx tree-sitter generate --abi 14`, copy `src/parser.c`,
`node-types.json`, `src/tree_sitter/parser.h` into the crate). Follow-up:
VS Code extension must register the `mox` language + `.mox` extension and
pass per-file parser initializationOptions; actor-internals validation
belongs to upstream rex-lsp.

Rosetta (Rune DSL) is the sibling primary model source (sigil-parsed,
bridged, auto-scored): integration guide at `docs/rosetta.md`, construct
matrix + defects ledger at `docs/rosetta-gap-analysis.md`.


## DDD design models as generator input (issue #449)

rexlang's Sculptor-style design layer: `.ddd` files compile — against their
imported `.mox` domains (which may `import schema` JSON / `import sigil`
rosetta) — into the versioned `rex_ir::ddd::DddModel` wire artifact
(format v1). `codegraph run --mox-files model.mox --ddd-files design.ddd
--config domains.toml --output out/` ingests the design (driver **Pass 1c**,
after the IFML pass) and the DDD generators become model-driven. Normative
docs: `docs/DDD.md` upstream; driver contract `rex_driver::compile_ddd_str`.

- **Import resolution**: imports resolve relative to the `.ddd` file
  (line-scan `import "x.mox"`, transitive mox via
  `ingest/rex_imports.rs` — the deduped home of `collect_domains`/
  `scan_imports`, shared with `ifml_actor_import.rs`); `import schema` via
  `scan_schema_imports`; `import sigil` content is the named file PLUS a
  candidate pool of every sibling `*.rosetta` (dot/`target` subtrees
  skipped, depth 16) — the rexlang CLI's directory-walk rule.
- **Hard error on design errors** (rosetta precedent):
  `Error::DddModel{file, reason}` with `rex_driver::render`-ed per-file
  diagnostics; a broken design never half-ingests. Class resolution is
  exact → `+type_suffix` → warn (the `wire_alias_refs` chain) at ingest;
  mox-unknown classes are rex compile errors (hard), graph-unknown classes
  ingest softly with `resolved_title = None` and skip mapping.
- **Graph plane** (rex-free): `DddModelGraph` + node families
  `DddApplication/DddModule/DddDesign/DddRepository/DddRepositoryOperation/
  DddService/DddSearch` in `codegraph-core/src/types/ddd.rs`; 7 edge types
  (`DddHasModule/Design/Repository/Service/Search/Operation`, 
  `DddBindsClass`); DDL in schema_ddl.rs. Trait surface is ONE composite
  pair (the `ingest_actor_policy` whole-artifact contract):
  `ingest_ddd_model(&DddModelGraph)` + `get_ddd_models() ->
  Vec<DddModelGraph>` (Mock + Grafeo + CachingQuerier). **Ordering
  contract**: nodes carry `ordinal`s; reads restore module-major
  declaration order (designs → services → searches per module) — the
  converter must walk rex modules in order. Signatures ride as rex-ir
  serde JSON (`type_json`/`return_type`), rehydrated by codegraph-generate
  (which may depend on rex-ir; codegraph-core may NOT). CachingQuerier
  caches `get_ddd_models`/`get_evt_models` (first-read-wins): every
  ddd-aware generator context rebuilds the design surface from these
  readbacks, and uncached they re-run several GQL queries + a whole-model
  JSON parse per entity per generator (~7,000 readbacks ≈ 80% of a
  9-package consumer's generation wall clock). Safe because the wrapper
  is created only in the generation phase — the graph is immutable there.
- **Mapping** (`codegraph-generate/src/ddd/design.rs`):
  `DddDesignSurface::from_graph(db)` indexes by resolved title. Design
  repository builtins become the TOP-priority ops source
  (`findById→read`, `findAll→list`, `save→create+update`, `delete→delete`;
  a design without a repository maps to an EMPTY op set) — override
  applied inside the ddd/ context builders (repository trait +
  `EntityTree`, dto, command, query) AND in `resolve_entity_operations`
  (api/api_model.rs, issue #449 parity). **Gating invariant**: every
  consumer of the op set must respect the EMPTY set — handler/command/
  query generators emit nothing, the router gates read/list routes too
  (`RouterEntity.has_read`/`has_list` — a template emitting unconditional
  routes points at handlers that were never generated), and the scaffold
  app_state/server templates gate the command-handler wiring on
  `entity.has_commands` (the query plane exists for every entity; the
  command plane only when a mutating op survives). Value designs and
  no-repository entities are the shapes that expose a non-gated consumer.
  Declared finders (`findByX(x)` Sculptor semantics: equality
  on the snake_cased param, single → `Option<E>`, multiplicity → `Vec<E>`)
  emit trait (repository.tera, whitespace-controlled design-gated block) +
  SeaORM impl (`EntityTree.design_finders`) + cornucopia query/adapter;
  non-lowerable finders warn+skip. The SeaORM binds go through `eq_arg`
  (repository_emitter/finders.rs): `ColumnTrait::eq` takes
  `impl Into<Value>` and sea-orm implements `From<T>` only for OWNED
  values, so `eq_arg` binds bare only Copy scalars (primitives, `Uuid`)
  and `.clone()`s everything else — params arrive crate-qualified
  (`chrono::NaiveDate`), so the test keys off Copy, not off non-Copy
  spellings. The Condition builder path spells `sea_orm::Condition` (the
  re-export), not `sea_query::`. `flags.auditable` WINS over the
  domains.toml/policy fallback (still ANDed with `!append_only`);
  `cache`/`nonPersistent` warn-and-record. Design
  `search` drives `has_fts` (text fields), `filter_fields`, and the FTS
  language (search default analyzer); boosts recorded only. DTO
  create/update emission follows design ops (`dto_response` always).
  Stereotype vs `is_entity` mismatch warns (mox owns entity/VO).
- **Sculptor semantics (rexlang #47, pin `a714070`)**: entity designs are
  `auditable` + `optimisticLocking` by DEFAULT — the artifact bools carry
  the effective decision, `!flag` opts out, negating default-off flags is
  a compile error. `protected` ops (service + repository) lower onto the
  graph (`is_protected` node prop, read-compat default false) but stay OFF
  the public API plane: `operations_for` excludes them, so `protected
  save` means no create/update endpoints while the repository still
  lowers. New validations (hard `Error::DddModel`): value designs demand
  `readonly` features, cross-aggregate `refers` to a non-root member is
  rejected, `scaffold` requires a repository. The two query built-ins
  lower onto the finder plane with consumer-known signatures
  (`DesignFinder.kind`): `findByKeys` → equality on every `id` feature
  (surrogate `id: Uuid` fallback), single row; `findByExample` →
  `Option`-wrapped equality on every stored non-id scalar feature (None
  filters out — SeaORM `Condition` builder, cornucopia
  `(:p IS NULL OR col = :p)` idiom), many rows; feature universe from the
  graph (`EntityFeatures::from_graph`), enum/class features excluded.
  `optimisticLocking` emission (version column + conflict check) remains
  a tracked deferral — recorded, no longer warned (default-on would warn
  on every design).
- **Gates**: `cargo test -p codegraph --test ddd_golden_tests` (committed
  rexlang `library.{mox,ddd,ddd.json}` fixture byte-compared in-process —
  the CLI-gates treatment, node-free) and `--test ddd_pipeline_tests`
  (end-to-end mox+ddd generation, determinism, and the
  config-driven-vs-design-driven BYTE-EQUALITY equivalence gate) plus
  `--test ddd_ingest_tests`. Flag-off byte-identity holds: no `.ddd`
  input ⇒ no nodes ⇒ unchanged output (repository.tera is the only
  template touched; the finder block is empty-loop-inert).
- **Deferred**: service `=>`/`inject` → handler/query wiring;
  `compile_ddd_str_with_actors` (capabilities recorded unvalidated, same
  as the rexlang CLI); optimistic-locking/cache emission; nonPersistent
  DDL suppression; search backends beyond Postgres FTS + document
  projections; per-field analyzers/boosts in the FTS plumbing; ddd on the
  classify/ifml-generate paths; LSP for `.ddd`.

## Events DSL as generator input (issues #454, #455)

rexlang `.evt` event contracts compile — against their imported `.mox`
domains — into the events plane: typed payload contracts, per-channel
publishers, per-subscription consumers, and the transport bindings behind
them. `codegraph run --mox-files model.mox --evt-files events.evt --config
domains.toml --output out/` ingests the contracts (driver **Pass 1d**, after
the ddd pass, part of the graph-cache inputs group) and the events
generators become model-driven; `--evt-files` also rides `codegraph doctor`
(`check_evt_files`).

- **Pipeline**: `ingest_evt_files`
  (`crates/codegraph/src/ingest/evt_ingest.rs`) reads each contract file,
  collects its transitively imported `.mox` domains (the shared
  `rex_imports` plumbing; imports resolve relative to the `.evt` file,
  `import schema`/`import sigil` content included), compiles with
  `rex_driver::compile_evt_str`, and hard-errors on any error-severity
  diagnostic (`Error::EvtModel`, the `Error::DddModel` precedent — a broken
  contract never half-ingests). Payload field types resolve against the
  ingested schema titles: exact → title + `type_suffix` → warn + count on
  miss; primitives resolve silently (never a warning, never counted).
- **Semantic IR (#455)**: `codegraph-generate/src/events/model.rs` —
  `EventArchitecture::from_graph` builds `events` (payload fields mapped to
  Rust: rex primitives, resolved schemas' `rust_type_name`,
  `serde_json::Value` on a miss) / `channels` / `domain_channels` /
  `subscriptions`. `PublicationSource::{Application, Database(Trigger)}`
  folds the CRUD-diff per-table trigger plane in as the implicit
  per-domain channels — a provenance-only rewire
  (`enumerate_event_trigger_tables` shares the ONE DDL-context
  enumeration with `DdlGenerator`; `domain_event_trigger.tera` output
  unchanged). `Transport::{Pgmq, Unbound}` + `capabilities()` —
  capabilities, not technologies: channels bind pgmq only on a
  `has_plpgsql()` target; sqlite runs leave them unbound (warned once per
  channel) and skip the persisted surface.
  `DeliveryPolicy::at_least_once_default()` = today's dispatch constants
  (5 retries / 10s exponential-backoff base / 30s visibility timeout).
  Queue naming is `events_{channel}` (codegraph convention); a channel
  named like a domain COLLAPSES into the domain queue — the domain
  migration band owns its creation, warned once.
- **Emission**: the presence-gated global `evt_events` generator →
  `src/events/{mod,contracts,emit,consumers}.rs` (one payload struct +
  `{EVENT}_VERSION` const per declared event; one event enum +
  `publish_{channel}` per pgmq-bound channel; one handler trait +
  dispatcher per subscription plus the process-wide registry). The Native
  superset envelope carries 11 keys (`event`, `version`, `channel`,
  `payload`, `event_type`, `domain`, `entity_table`, `entity_id`,
  `tenant_id`, `occurred_at`, `correlation_id`) with `event_type` = the
  declared event name so `find_webhook_subscriptions` matching is
  unchanged. The webhook drain dispatches through the registry
  (`dispatch_registered(&self.db, &message) == 0` delete condition),
  `webhook_api.rs` gains vocabulary validation (declared events ∪
  created/updated/deleted), and the scaffold's graph-driven `has_events`
  gates `pub mod events;` + the gated `anyhow` in lib.rs/Cargo.toml.
- **Gates**: `cargo test -p codegraph --test evt_golden_tests` (the
  committed rexlang `orders.{mox,evt,evt.json}` conformance fixture
  byte-compared in-process — the CLI-gates treatment, node-free),
  `--test evt_ingest_tests` (conversion/resolution/hard-error semantics +
  the inputs group), and `--test evt_pipeline_tests` (end-to-end mox+evt
  generation through `driver::run`, determinism, and the flag-off
  contract: no `.evt` ⇒ no `src/events/` and byte-identical common tree
  modulo the gated webhook files + module index). Flag-off byte-identity
  is pinned in-crate by `src/evt_golden/*` (dispatch/api_endpoints/lib/
  cargo_toml) + `src/events/pgmq_setup_pre_evt_template.rs` + the canaries.
- **Deferred**: CloudEvents envelope variant; delivery-policy DSL syntax
  (upstream rexlang #31 slice 2); deployment-profile transport bindings
  (#455 §13); full Connector SPI (#455 §14); workers-topology per-channel
  drain (monolith-first this PR — per-domain worker drains only read
  `events_{domain}`); publisher/aggregate binding + event domain
  attribution (rexlang issue drafts, see below); LSP for `.evt`.

## Deployment models as generator input (rexlang #49)

rexlang `.deploy` deployment models compile — against their imported `.ddd`
designs and `.ifml` flows — into the deployment plane: applications of
components (kinded: api/worker/database/queue/objectStore/frontend) and
`connects` edges, profiles (closed targets incl. `cloudflareWorkers` +
defaults/policies/mappings), and deployments. `codegraph run
--deploy-files fleet.deploy ...` ingests them (driver **Pass 1e**, after
the evt pass, part of the graph-cache inputs group — a `.deploy` edit
busts `.graph-cache/`); `--deploy-files` also rides `codegraph doctor`
(`check_deploy_files`, plus two cross-checks: a `cloudflareWorkers`
profile ⇔ the resolved `deployment_topology = "workers"`, and per-domain
component coverage — the second warns, the first is hard).

- **Pipeline**: `ingest_deploy_files`
  (`crates/codegraph/src/ingest/deploy_ingest.rs`) reads each deployment
  file, resolves its imports relative to the file (`.ddd` designs compile
  through the shared `read_and_compile_ddd` union path, `.ifml` flows via
  `parse_ifml_file`), compiles with `rex_driver::compile_deploy_str`, and
  hard-errors on any error-severity diagnostic (`Error::DeployModel` — a
  broken deployment model never half-ingests).
- **Storage**: ONE `:DeployModel` node per source file — `source_path` +
  `model_json` (the whole `codegraph_core::types::DeployModelGraph` typed
  mirror serialized; consumers always want the model whole, so the
  evt/ddd per-family node split buys nothing here). Upsert-by-source_path;
  readback deserializes + sorts by `source_path`
  (`ingestor/nodes_deploy.rs`, `querier/deploy.rs`).
- **Derivation** (workers scaffold,
  `codegraph-generate/src/scaffold/worker.rs`): `DeployShape` resolves the
  first `cloudflareWorkers` profile → its first deployment → that
  application; absent/unresolvable = pure config behavior (flag-off
  identity). Component ↔ worker bridges BY NAME (a domain-named api/worker
  component is that domain's worker); `connects` edges REPLACE the
  `service_bindings_or_depends()` fallback per domain (explicit
  `domains.toml` `service_bindings` still win; edges to non-domain
  components are skipped); an api component named `gateway` narrows the
  gateway's `[[services]]` to exactly the domains receiving a
  `connects gateway -> X` (no gateway component ⇒ bind-all, byte-identical
  template output). Composition children whose
  `child_table_name(parent, field)` collides with their own `pg_table_name`
  hoist the parent's child-table column shape into their standalone
  migration (the `GraphQuerier::composition_table_conflict` plane); the
  parent's inline block still adds the FK/index to the now-existing table.
- **Gates**: `cargo test -p codegraph --test deploy_ingest_tests`
  (conversion/hard-error/cache-input semantics), the grafeo round-trip
  tests in `querier/deploy.rs`, the doctor cross-check tests in
  `init_tests.rs`, and the scaffold derivation tests in
  `scaffold/worker.rs`.
- **Workerd parity** (from the first full consumer run — hr-platform never
  exercised these): the gateway carries a CORS layer under the monolith's
  `CORS_ALLOWED_ORIGINS` contract (dependency-free `gateway/src/cors.rs`,
  native middleware + wasm fetch-boundary handling — the inbound wasm body
  is `worker::Body`, so `from_fn` cannot span the seam); wrangler's
  `[build]` assumes `cargo install worker-build` as a prerequisite (the
  per-worker reinstall cost three minutes per worker); `pub mod entity`
  (SeaORM) is `#[cfg(feature = "native")]` — SeaORM cannot link wasm32 and
  nothing in the wasm slice references it; cornucopia SELECT columns alias
  to the Rust field names and findByExample headers mark optional params
  with `?` (nullability is header-driven, and bare `:p IS NULL` references
  are indeterminate at prepare time).
- **Deferred**: crons/observability/webhook queues/hyperdrive names as
  `.deploy` component settings (needs rex-driver setting-vocabulary
  extensions); `queue`/`objectStore` components driving worker queues;
  `remote_include_mode = "http"` over service bindings; non-workers
  targets (standalone/dockerCompose/kubernetes backends do not exist yet
  — rexlang #49 slice 2).

## Namespaces as first-class citizens (issue #267)

Namespace = where a type lives + what it can see (hierarchical, dotted,
import-based); domain stays the bounded-context/deploy boundary. Core-only
slice — producers (mox/rosetta/JSON-`$id`) connect in #268.

- **Naming-collision resolution**: the AT-Protocol line previously owned
  `NamespaceNode` with unrelated repo-namespace semantics. It is renamed
  `AtprotoNamespaceNode` (`types/atproto.rs`), with trait methods
  `ingest_atproto_namespace`/`get_atproto_namespaces` and grafeo label
  `:AtprotoNamespace`. The graph-wide namespace concept owns the canonical
  names: `NamespaceNode { fqn, parent, source }` (`types/namespace.rs`),
  `:Namespace` label, `ingest_namespace`/`list_namespaces`.
  `EdgeType::InNamespace` is SHARED by both families (grafeo match uses a
  `WHERE a.nsid = … OR a.schema_id = …` form).
- **Model**: `SchemaNode.namespace`/`SchemaClassificationData.namespace`
  (`#[serde(default)]`, read-compat with old payloads); schema_id contract =
  `<ns>::<Name>` when namespaced, legacy id verbatim otherwise
  (`qualified_schema_id`); title uniqueness is scoped per namespace with
  collision policy first-plain → namespace-qualified → numeric suffix
  (`disambiguate_schema_ids`).
- **Edges**: `InNamespace` (Schema → Namespace, shared),
  `NamespaceParent` (child → parent; also persisted flat on the node),
  `NamespaceImports` (`EdgeProperties.import_wildcard`/`import_alias`),
  `NamespaceDepends` (derived via `derive_namespace_depends` through the
  domain `depends_on` plane; enum + DDL exist, nothing ingests it).
- **Graph plumbing**: GraphIngestor `ingest_namespace`/`ingest_namespace_import`;
  GraphQuerier `list_namespaces`, `list_schemas_by_namespace(fqn, recursive)`,
  `get_namespace_imports`, `namespace_generation_order` (deterministic Kahn:
  imported-before-importer, lexicographic fqn tie-break, cycle = error naming
  members — `topological_namespace_order`). Mock + Grafeo + CachingQuerier
  all implement them.
- **Config** (`domains.toml`): `[namespaces."cdm.base.datetime"]` with
  optional `domain = "…"`. BOTH TOML spellings normalize to the same FQN
  (quoted flat key or nested unquoted tables — nested intermediates are path
  segments, not declarations; a level is declared when it has `domain` or is
  a leaf). `domain` is reserved at every level; unknown scalar keys and
  malformed fqns are parse errors. Discovered-but-undeclared namespaces are
  allowed — the declared set is only the validation baseline.
- **Validation** (`validate.rs`, no-op when the graph has no namespaces):
  `namespace_import_undeclared` (Error — target not in graph/allowlist),
  `namespace_import_undeclared_dependency` (Error — cross-domain import
  without `depends_on`, mirrors `fk_target_undeclared_dependency`;
  namespace→domain resolution: config `domain` wins, else unique member-schema
  domain), `namespace_domain_conflict` (Warning — declared domain vs observed
  member-schema domains disagree).
- Back-compat is pinned: namespace-less graphs trigger zero namespace checks,
  `SchemaNode.namespace` stays `None`, and generated output is unchanged.
  Gate: `cargo test -p codegraph --test namespace_tests`.

### Source bridging + namespace-aware generation (issue #268)

Producers now populate the #267 plane; generation consumes it behind the
`namespace_layout` gate.

- **mox** (`ingest/mox_ingest.rs`): `package <dotted.name>` → NamespaceNode
  (source `"mox"`) + dotted `NamespaceParent` chains + `SchemaNode.namespace`
  + `InNamespace` edges for every bridged class/enum schema. The rex grammar
  REQUIRES a package, so every compilable mox model is namespaced; the mox
  equivalence gate pins that this changes NO generated output (flat layout is
  namespace-inert). New `MoxIngestStats::namespaces` counter (displayed only
  when non-zero).
- **rosetta** (`ingest/rosetta_ingest/`): `namespace a.b` → NamespaceNode
  (source `"rosetta"`); `import a.b.*` / `import a.b as x` → NamespaceImports
  edges (wildcard/alias payload; the lowered `imported_namespace` string
  embeds `.*` — strip it). Import-only targets (e.g. the sigil builtins'
  `com.rosetta.model`) land as `"discovered"` nodes so import edges resolve
  and #267 validation sees them — a cross-domain import without
  `depends_on` is a hard validation error (the rosetta_bridge fixture's
  domains.toml declares one). Schemas carry `namespace` + `InNamespace`.
  `RosettaIngestStats::{namespaces, namespace_imports}` now count ingested
  nodes/edges.
- **JSON** (`ingest/async_ingest.rs`): a schema joins a namespace ONLY when
  it declares one — `$namespace` verbatim, else `$id` path-derived
  (`https://cdm.example/cdm/base/datetime/Foo.json` → `cdm.base.datetime`;
  host skipped, filename dropped; bare-host/filename-only/`urn:` ids → None).
  No declaration ⇒ namespace-less ⇒ byte-identical back-compat (NOT a
  domain-name default). Source `"json"`; inline `#/$defs` children inherit
  the parent's treatment (they generate as its children). Classification
  scoring is unchanged — `classify_domain` operates over domain-assigned
  schemas that may span namespaces (`SchemaClassificationData.namespace`
  rides through).
- **Generation order** (`codegraph-generate` `compute_generation_order`):
  when the graph has namespaces, per-domain emission order ranks titles by
  `namespace_generation_order` (imported-before-importer, namespace-less
  last, title tie-break) and the title-claim key becomes `(namespace, title)`
  — same title in two namespaces are two types. Namespace-less graphs take
  the exact pre-#268 path (byte-identical). An import cycle is a hard
  `Error::Config`.
- **`namespace_layout` gate** (profiles.toml `[features]`, default OFF =
  flat/byte-identical): helpers `namespace_module_path`/`namespace_module_rust`
  in codegraph-core (`cdm.base.datetime` → `cdm/base/datetime` /
  `cdm::base::datetime`), threaded via `BuildPlan.namespace_layout` →
  `ProjectConfig.codegen.namespace_layout`. When ON and the schema carries a
  namespace: `sea_orm_entity` emits `src/entity/{ns}/{module}.rs`, `dto` +
  `dto_included` + `repository` emit under `src/domain/{ns}/{module}/`, the
  repository emitter references `crate::entity::{ns}::{module}` and
  registers/imports DTO types on namespace-derived module paths
  (type_registry keeps handler imports coherent). SvelteKit/API URL
  segments stay title/api_path_segment-based — namespaces are NOT URLs.
  Deferred (audit list): handler/app_state/query/command template-level
  `crate::domain::{domain}::…` strings, child entity file paths, include
  TARGET namespace resolution, cornucopia/grpc/openapi paths.
  Gates: `cargo test -p codegraph --test namespace_bridge_tests`,
  `namespace_graph_parity_between_equivalent_json_and_mox_models` in
  `mox_equivalence_tests.rs`.

## IFML Integration

### Overview

IFML (Interaction Flow Modeling Language) DSL integrated alongside JSON Schema as a
**complementary primary input**. JSON Schema defines the data model (entities/fields),
the IFML DSL defines the interaction model (views/navigation/events). Both feed into
the same Grafeo graph, linked by data binding edges. The authoring loop:
`codegraph ifml-scaffold` emits a starter `.ifml` from schemas → user edits it
(LSP validates `data:`/`fields:`/navigate targets/modules) → `codegraph ifml-generate`
renders behavior-wired SvelteKit pages + Playwright tests. Component quality parity is
achieved by mapping IFML elements to handcrafted components via `ifml-components.toml`.

### Architecture layers

Note: the IFML generators live in `codegraph-generate` (moved out of the `codegraph`
crate); older docs referencing `crates/codegraph/src/generate/ifml/` are stale.

| Layer | Location | Technology |
|-------|----------|------------|
| **DSL Parser** | rexlang `crates/rex-ifml` (git dep, rev-pinned) | Pest (Rust PEG parser), consumed via `use rex_ifml::*` |
| **AST types** | rexlang `crates/rex-ir/src/ifml.rs` (re-exported by `rex-ifml`) | Serde-serializable AST + `render_expression()` (camelCase, type-tagged wire format) |
| **Grammar** | rexlang `crates/rex-ifml/src/grammar/ifml.pest` | PEG grammar (source of truth) |
| **Tree-sitter grammar** | `codegraph-vscode/grammar/grammar.js` → `crates/tree-sitter-ifml/src/parser.c` | LSP/editor parsing; regenerate with `npx tree-sitter-cli generate --abi 14` |
| **Graph model** | `crates/codegraph-core/src/types/ifml.rs` | 7 node types, 16 edge types, `NavigationFlowRecord`, `ModuleUseRecord` |
| **Grafeo DDL** | `crates/codegraph-grafeo/src/schema_ddl.rs` | GQL CREATE statements |
| **Grafeo ingestor** | `crates/codegraph-grafeo/src/ingestor/` | GQL INSERT for IFML nodes (extra node props like `conditional_expression`/`module_uses`/`roles` persist without DDL changes) |
| **Grafeo querier** | `crates/codegraph-grafeo/src/querier/` | GQL MATCH queries for IFML (nav flows resolve component → owning ViewContainer) |
| **GraphIngestor trait** | `crates/codegraph-core/src/traits/ingestor.rs` | 6 IFML ingest methods |
| **GraphQuerier trait** | `crates/codegraph-core/src/traits/querier.rs` | 9 IFML query methods (incl. `get_ifml_action_triggers`, `get_parameters_for_view`) |
| **CachingQuerier** | `crates/codegraph-core/src/caching_querier.rs` | Delegates IFML queries |
| **Ingestion bridge** | `crates/codegraph/src/ingest/ifml_ingest.rs` | AST → GraphIngestor (`IfmlIngestStats` incl. module_uses/actors) |
| **Scaffold CLI** | `crates/codegraph/src/ifml_scaffold.rs` | schemas + classifier → starter `.ifml` (parse-verifies its own output) |
| **IfmlQuerier** | `crates/codegraph-generate/src/ifml/querier.rs` | Lossless model assembly (real event actions, params, bindings) |
| **Dependency sort** | `crates/codegraph-generate/src/ifml/dependency_graph.rs` | Kahn's algorithm (drives emit order) |
| **Route generator** | `crates/codegraph-generate/src/ifml/route_generator/` | Behavior-wired SvelteKit pages (events → goto/submit handlers, testids) |
| **Nav generator** | `crates/codegraph-generate/src/ifml/navigation_generator.rs` | Route map + type helpers |
| **API path resolution** | `crates/codegraph-generate/src/ifml/api_paths.rs` | Entity → real endpoint (API model > ApiResource > legacy guess) |
| **E2E generator** | `crates/codegraph-generate/src/ifml/e2e_test/` (module split: `assembly`/`fixtures`/`render`/`spec_payload`/`ux_plans`/`pom`/`pom_render`/`kernel`) | Playwright specs (render/click-through/validation/CRUD) + the POM (kernel + per-view page classes) |
| **Component mappings** | `crates/codegraph-config/src/ifml_components.rs` | `ifml-components.toml`: IFML element → handcrafted component |
| **Templates** | `crates/codegraph-generate/templates/ifml/` | Per-framework `page.tera`/`page_load.tera`/`navigation_map.tera` |
| **Profile caps** | `crates/codegraph-generate/src/ifml/profiles.rs` | `ifml_backend` + `ifml_route_{fw}`/`ifml_navigation_{fw}`/`ifml_e2e_test_{fw}` |
| **LSP server** | `crates/codegraph/src/lsp/` | lsp-server crate; diagnostics (unknown entity/field/navigate target/module), completions |
| **CLI** | `crates/codegraph/src/cli.rs`, `main.rs` | `ifml-scaffold`, `ifml-generate`, `--ifml-files`, `--ifml-components` |

The `ast-ifml` AST (`crates/ast-ifml/src/generated/mod.rs`) is committed and is the
source of truth; plain builds never regenerate it. Codegen is opt-in: run
`AST_GEN=1 cargo build -p ast-ifml` only after changing the IFML grammar
(`crates/tree-sitter-ifml`), then commit the regenerated file (issue #221).

### IFML DSL syntax (C-like)

```ifml
domain "sales" { schema "sales"; }

actor "Admin" { role: admin; }

view "CustomerList" {
    label "Customer Management";
    landmark: true;
    roles: [admin, manager];

    component "grid" {
        type: list;
        data: Customer;
        fields: [name, email, phone, status];

        on select(row) if row.active == true -> navigate("CustomerDetail", { customerId: row.id });
    }
}

view "CustomerForm" {
    params { id: Uuid, slug: String = "home" };

    component "editor" {
        type: form;
        data: Customer;
        field title -> input text {
            required: true;
            validations: [len(title) > 2];
            messages: ["Title too short"];
        }

        on save -> navigate("CustomerList");
    }
}

view "Dashboard" {
    use "Pagination" as pager { page_size: 25 };
}
```

Guards (`if <expr>` on views/components/events), validation messages (positionally
paired with `validations`), param defaults, module instantiation and actors all parse,
persist to the graph, and flow into generation (messages → `data-validate-message`,
defaults → load-fn fallbacks). Actor declarations are parsed/counted only — no node
type yet; role-based route-guard codegen is deferred.

Module composition (rexlang pin `a714070`): `ModuleDeclaration.module_uses`
(`use "Other" as alias { overrides };` inside a module body) ingest as
`ModuleDefinitionNode`s (`ingest_module_definition` — trait + Grafeo +
Mock; internal uses ride the node prop AND module→module
`HasModuleDefinition` edges, view-level uses gain resolvable view→module
edges), counted in `IfmlIngestStats.module_uses`; `get_ifml_modules` reads
them back (trait + Grafeo + Mock + CachingQuerier). Deep navigation
(`a.b.c` folded `FieldAccess` chains) parses and composite `filter:`
expressions persist in canonical DSL source (`render_value_expression` —
identifiers stay verbatim).

### IFML node types (Grafeo graph)

| Node | Purpose |
|------|---------|
| `ViewContainer` | A screen/page with views, params, components |
| `ViewComponent` | A UI element (list, form, details) with data binding |
| `Event` | User or system event triggering navigation/actions |
| `Action` | Business logic invocation |
| `ParameterDefinition` | In/out/inout params on views |
| `DataBinding` | Connection to JSON Schema entities |
| `ModuleDefinition` | Reusable interaction pattern |

### IFML edge types

`ContainsViewContainer`, `ContainsViewComponent`, `HasEvent`, `NavigationFlow`,
`DataFlow`, `HasParameter`, `HasDataBinding`, `BindsToEntity`, `BindsToProperty`,
`TriggersAction`, `ActionEvent`, `HasModuleDefinition`, `HasConditionalExpr`

### IFML DSL to graph flow

```
JSON Schema → ifml-scaffold (CLI) → starter .ifml → user edits (LSP)
.ifml file → Pest parser → AST → GraphIngestor (GQL INSERT) → Grafeo graph
                                                                    ↓
JSON Schema → SchemaLoader → GraphIngestor (GQL INSERT) → Grafeo graph
                                                                    ↓
                    IfmlGraphQuerier (lossless: actions/params/bindings)
                                                                    ↓
              route_generator (behavior-wired pages) + e2e_test (Playwright)
```

### Component mappings (`ifml-components.toml`)

`--ifml-components <file>` (on `generate`/`run`/`ifml-generate`) maps IFML elements to
handcrafted components so generated pages reach parity with the entity-scoped UI
pipeline. Resolution priority: **component name → type → semantic role → kind**
(`table`/`form`/`details`/`chart`), optionally scoped with `view = "Name"`:

```toml
[[component]]
kind = "table"
path = "$lib/components/DataTable.svelte"
export = "DataTable"
testids = { root = "data-table", row = "data-row" }
```

Mapped components render as import + invocation with conventional props (`data`,
`fields`, testids); unmapped components fall back to built-in templates. Fallback
markup carries stable selectors: `{component}-{table,row,form,submit,error,details}`
— the e2e generator's contract.

### Semantic UI layer (issue #196)

The mapping ontology includes a closed `SemanticRole` enum (strict TOML validation):
`action-control`, `navigation-control`, `field`, `selection-field`, `collection`,
`modal-view`, `presentation-container`, `display`, `shell`, `pagination`. The route
generator computes a role per slot (form save/cancel buttons → `action-control`,
dropdown/radio inputs → `selection-field`, `modal: true` views → `modal-view`, xor
containers → `presentation-container`, landmark views → `shell`, paginated lists →
`pagination`), and mappings match on it between the name and kind tiers.

Design-system packs: `--ifml-design-system shadcn-svelte` (or
`ifml_design_system = "shadcn-svelte"` in profiles features; CLI > profile > none)
loads a built-in pack (`crates/codegraph-config/src/packs/shadcn-svelte.toml`,
embedded via `include_str!`) covering all 10 roles. Project `--ifml-components`
entries are merged BEFORE pack entries, so they shadow pack entries per tier; pack
entries fill gaps. Unknown pack names error strictly. No-pack output is byte-identical
to pre-ontology output (all role-driven rendering is mapping-gated).

Mapping-gated generation beyond whole components: mapped `action-control` buttons
(`<Button onclick={submit_editor}>`), `modal-view` wrappers (`<Dialog bind:open>` +
`&dialog=open` nav params + close handler), `presentation-container` wrappers around
xor containers, and a `+layout.svelte` nav shell emitted from landmark views when a
`shell` mapping resolves. Control inference from domain types lives in
`crates/codegraph/src/ifml_control_inference.rs` (codelist → dropdown + selection-field
with values, entity-ref → dropdown + options note, email/password/number/datetime/uuid
heuristics) — used by `ifml-scaffold`; route-generator unification is a follow-up
(generate crate lacks classifier deps). Known limitations: nested containers flatten
into separate view containers (no Tabs grouping), layout nav hrefs keep event-scoped
binding expressions verbatim.

### Workflow + authorization-driven UI (phase 4)

View `roles: [...]` thread from the graph into the load context; views with roles
emit a SvelteKit `redirect(303, '/')` guard in `+page.ts` checking `currentRoles()`
against `viewRoles`, plus a one-time `src/lib/roles.ts` helper (never overwritten)
reading `globalThis.__USER_ROLES__` (real auth sets this, e.g. from `+layout.ts`
server data). Components bound to entities with a `domains.toml` workflow
(`WorkflowConfig`) render `data-workflow-state`/`data-workflow-terminal` badges
(`{component}-state` testid) across list/details/form markup, and the e2e generator
emits `{view}.workflow.spec.ts` (API fixture → initial-state assertion), mirroring
the entity pipeline's workflow test convention. Both are presence-gated: no roles /
no workflow → byte-identical output. Deferred: transition buttons, mapped-component
badge parity, DSL-level state guards.

### rexlang authorization integration (issue #199)

The IFML DSL supports `import "<path>.actor";` (top-level, `.ifml`-relative) and
view `requires: [Capability, ...];`. codegraph depends on the rexlang crates
(`rex-ir`, `rex-driver` as workspace path deps — git-pin at release): policy
parsing/typechecking stays in rexlang, codegraph consumes the typechecked
`ActorModel`. `crates/codegraph/src/ifml_actor_import.rs` resolves `.json`
artifacts (`ActorModel::from_json`) or `.actor` sources
(`rex_driver::compile_actors_str` with transitively collected `.mox` domains),
merges+dedups imports, and ingests via `ingest_actor_policy` (diagnostics warn,
never fail generation; `imported_policies` stat).

Graph model (`crates/codegraph-core/src/types/authorization.rs`): `ActorNode`
(`kind: human|agent`, `extends`), `CapabilityNode` (class bound), `GrantEdge`
(permit/forbid + `when_expr` + obligations JSON), `ActorPolicy` singleton
(blocks + never_both); DDL in schema_ddl.rs (`when` is reserved → `when_expr`).
`resolve_effective_permits` walks `extends` chains — forbid wins over permit,
duplicates collapse, deterministic order.

Guards (capability primary, roles compat): views with `requires`/`roles` emit a
two-check `+page.ts` guard — `can(c)` from `ROLE_CAPABILITIES` (policy-derived,
generation-time effective permits) ∪ `globalThis.__USER_CAPABILITIES__`, plus the
roles check — denial redirects to the first unguarded view. `src/lib/roles.ts`
(one-time) carries `currentRoles()` + `can()`. Controls in guarded views gate
behind `{#if}` matching the load guard exactly (whole-component invocations never
wrapped; unguarded views byte-identical). LSP warns on unknown capabilities/actors
when the document's imports resolve to a policy. E2E emits persona tests per human
actor (`addInitScript __USER_ROLES__`). Deferred: react/vue/flutter guard parity,
role-conditional per-control capabilities, delegation/purpose persistence
(currently dropped in conversion).

### Reverse inference: `codegraph ifml-derive` (phase 5 spike)

`codegraph ifml-derive --from-svelte <dir> [--output app.ifml]` parses SvelteKit
`.svelte` pages (`tree-sitter-svelte-next` for markup; string scanning for script
handlers) and infers an IFML model: routes → views, `<form>`/inputs/selects → form
fields, `onclick`/`on:click` handlers → events (save/cancel/click heuristics),
`goto()` → `navigate()` with identifier bindings, confident single-segment `fetch()`
→ `data:`. Output is parse-verified like `ifml-scaffold`; skips (nested containers,
unnamed controls, non-confident fetches, non-page files) are reported on stderr.
Known spike limitations: `function NAME` handlers only (arrow consts fall back to
`action()`), flat one-level inference, best-effort target naming, naive
singularization, plain/dotted-identifier bindings only, no `type: list` inference
(row clicks become view-level `select(row)` events).

### IFML Playwright tests

`ifml_e2e_test` (profiles.toml ui section, expanded per framework) emits
`{fw}/tests/ifml/{view}.spec.ts`: render tests always; click-through (per
NavigationFlow, API-created fixtures, `waitForURL` with bound params), validation
negatives and CRUD round-trips only when the bound entity is schema-backed (runs
without schemas degrade to render tests). Emitted `playwright.config.ts`/`package.json`
never overwrite existing files; the config gains env-gated Bearer auth
(`IFML_API_KEY` → `extraHTTPHeaders`) for runs against an authenticated API.

### IFML app skeleton (`ifml-skeleton` generator)

`IfmlSkeletonGenerator` (Global, `ifml_skeleton`/`ifml_skeleton_{fw}` capabilities;
runs first via `GlobalGenerator::sequential_first()` so the e2e generator's
if-absent stubs never clobber it) emits a buildable SvelteKit app skeleton
never-overwrite: `package.json` (svelte ^5.56, kit, vite, adapter-auto,
svelte-check, typescript, @playwright/test; dev/build/preview/check/test:e2e
scripts), `vite.config.ts` (`sveltekit()` + `/api` proxy → `IFML_API_ORIGIN`
env, default `http://127.0.0.1:3000`), `svelte.config.js`, `tsconfig.json`,
`src/app.html`, `src/app.d.ts`. Svelte-only; other frameworks keep the
e2e-generator stubs. Forms branch at runtime: id-param views emit
`const isEdit = !!viewParams[<param>]` → POST collection (create) vs PUT item
(update); details fallback reads `data.item.<field>`.

### IFML validation gate (full-stack)

`crates/codegraph/tests/ifml_codegen_gate.rs` — TDD acceptance gate that runs the
FULL pipeline over the kitchen-sink fixture (`tests/fixtures/ifml_gate/`: shadcn
pack, modal view, requires/roles + rexlang `policy.actor`, workflow, codelists)
into `target/ifml-gate/` and asserts the generated app works against a real
backend: T0 migrations apply + axum boots (`/health`), T1 `svelte-check` zero
errors, T2 `vite build`, T3 Playwright specs pass (render/click-through/
validation/CRUD round-trip/create-POST-persists/details values/persona
allow+deny/workflow/ux — every category driving the app through the POM
page classes, zero raw testids in spec bodies) against the API through a
vite `/api` proxy, T4 view-removal + regen stays green (including the
stale-POM pin: the removed view's `tests/pages/{view}-page.ts` must be
swept). Reusable harness lives in `tests/test_framework/`
(`NodeProject`, `postgres.rs` GateDb, `axum_server.rs`, `playwright.rs`,
`extras.rs` — ui stubs + gate playwright config + key-injecting proxy; the
SvelteKit skeleton itself is generator-provided). Run:

```bash
cargo test -p codegraph --test ifml_codegen_gate -- --ignored --nocapture
```

Requires node 22 + chromium (auto-installed) + Postgres (`DATABASE_URL`, default
`postgres://postgres:postgres@localhost:5432/postgres`; a `postgres:16` docker
container on 127.0.0.1:15432 is bootstrapped as fallback). Unique DB per run,
dropped on finish; warm run ~110s. Runs locally and on the nightly
`.github/workflows/ifml-gate.yml` (workflow_dispatch + 03:00 UTC cron, postgres:16
service, gate logs artifact on failure) — PR CI stays node-free.

### IFML doctor gate (issue #468 phase 1)

`--ifml-files` rides `codegraph doctor` (`check_ifml_files` in
`crates/codegraph/src/init/commands.rs`): each file goes through the same
`rex_ifml::parse_ifml_file` the ingest path uses (parse error = hard
failure), and its `import "….actor"` statements resolve read-only through
`resolve_actor_policy`. The asymmetry is the point — the run path
warns-and-continues past an unresolvable policy import (the view guards
would silently lose their policy), so doctor makes that a hard failure
instead. `.mox` imports on `.ifml` files are checked for existence but are
not policy business.

## Generated integration testing (#463)

Every generated app ships its integration tests: hurl API contracts +
a generated authn/authz suite (API), and IFML Playwright specs whose POM
carries real test-side auth (UI). Locked decisions: hurl only (the
cosmos-layout `playwright-ts` generator is retired), test-side IFML auth
(per-persona API keys via `public.create_api_key` — no login UI), in-app
Rust integration tests deferred.

### `hurl_contract` domain generator

`crates/codegraph-generate/src/api/hurl_contract.rs` +
`templates/hurl/{entity,auth,rls_isolation}.tera`; capability
`hurl_contract` ON in default/fullstack (+enterprise, + the init scaffold
template). Per-entity `{nn:02}_{domain}_{entity}.hurl` numbered from 10 in
entity generation order (codelists are seed tables, not contract surface —
they don't number and can't anchor): LIST envelope, CREATE 201 + `data.id`
capture (required FK parents created first via preceding POSTs with their
own captures), GET field echo, zero-uuid 404, PUT roundtrip, DELETE 204 +
GET 404 — `{{api_key}}` everywhere, minimal required-field bodies
(codelist/inline-enum fields use the ref target's EnumValues via
`codelist_enum_name_from_ref`, keeping JSON and mox runs byte-identical).
Once per run (the domain owning the FIRST entity) the authn/authz suite:
`01_auth.hurl` (401s), `03_scope_denial_403.hurl` (read-only key write →
403 `INSUFFICIENT_SCOPE`), `04_cross_tenant_404.hurl`, and
`08_rls_isolation.hurl` — the ops stage-8 file, named in the manifest
`[hurl].skip`. The ops generator emits the `[hurl]` manifest section
exactly when the plan carries the generator (`with_hurl_contracts`);
org ids MUST stay aligned with `OpsHurl::default()`. The api suite warns
when a hurl dir yields zero runnable files. Hurl variables never appear
as Tera literals — URLs/headers/bodies/asserts are precomputed context.
Gates: `cargo test -p codegraph --test hurl_contract_tests` (byte-golden +
the `[hurl]` OpsManifest parse coupling, node-free);
`--test review_api_suite -- --ignored` is the live acceptance gate
(regenerate fixture → `testkit api` → app_user pool mode + 403 + 404 over
real HTTP; the fixture manifest carries `[hurl]`, and the xfail tolerance
for the 500-mapping gap is GONE — any failure is hard).

### IFML test-side auth + journeys

Feature `ifml_e2e_auth` (default/fullstack ON, BuildPlan →
`ProjectConfig.codegen.ifml_e2e_auth`; presence-gated — flag off ⇒
byte-identical, inline `addInitScript` persona stubs stay). New
`crates/codegraph-generate/src/ifml/e2e_test/auth.rs`: `tests/e2e/auth.setup.ts`
(globalSetup provisioning one API key per human-actor persona, writing
`.auth/{persona}.json` with apiKey/roles/capabilities; view-`roles`
personas are the no-policy fallback), `tests/e2e/personas.ts`
(`test.use` storage-state + Bearer + the single
`__USER_ROLES__`/`__USER_CAPABILITIES__` seeding place),
`{view}.auth.spec.ts` (denial = redirect AND presentation, capability
control-gating), and `tests/journeys/*.journey.spec.ts` (workflow handoff
across personas via the POM transition map + shell-nav link clicks).
POM extensions: `{comp}DeleteViaMenu()`/`{comp}Cancel()` page-class
methods; the emitted `playwright.config.ts` carries `globalSetup` when
auth exists. Driver note: profile-driven `run` injects CLI
`--ifml-framework` targets when the profile declares none (the
`ifml_generate` fallback) — without it no IFML artifacts are emitted at
all. Nightly gate: `auth` + `journeys` T3 categories in
`ifml_codegen_gate.rs`. Gate: `cargo test -p codegraph --test
ifml_auth_setup_tests` (5 pins, node-free).

### Testing layers

Two layers per the TDD discipline: (a) node-free PR-CI pins
(`hurl_contract_tests`, `ifml_auth_setup_tests` — byte-goldens/contains
over generated artifacts); (b) the nightly live-app layer
(`review_api_suite` over real Postgres + hurl + axum; the `auth`/
`journeys` gate categories over a real browser through the POM). PR CI
stays node-free/DB-free. The playwright-ts retirement removed
`playwright_ts_*` capabilities/generators and the review fixture's stale
`e2e-tests/`; the wired `playwright-entity`/`playwright-global` (Rust
fixture crate) are untouched.

## UX Rules (#286 ux-rules branch)

Codified UX rules for generated UIs ("form follows data"): a ux-rules TOML
document (built-in `ux-default` pack, optionally shadowed by a project
file via `--ux-rules <file>`) plus Pass-1 dimension inference resolve into
a per-entity `UxPlan`; the entity UI pipeline and the IFML pipeline both
render it. Four passes: dimension inference → rules/ordering → action
tiering → invisible-UI visuals, plus a sort plane (`?sort=` allow-list)
and advisory diagnostics (money hints, timeline suggestions, overflow
accounting — printed as `warning: ux-rules: …`, never fatal). Timeline
rendering is strictly opt-in via `[[collection]] display = "timeline"`.
Full reference, the 34-row #286 traceability table, testid contract, and
the deferred ledger: `docs/ux-rules.md`.

| Module | Role |
|---------|------|
| `crates/codegraph-config/src/ux/` | Config plane: `dimension.rs` (8 kebab dimensions), `presentation.rs` (Display/Align/FormatConfig/ToneMap), `rule.rs` (ColumnRule/CollectionRule/ActionRules + `glob_match`), `mod.rs` (strict parse with `[[column]] #N`-attributed errors + hint lines, `BUILT_IN_UX_PACK`, `merge`), `packs/ux_default.toml` (en-NZ/NZD pack, doubles as the key reference) |
| `crates/codegraph-generate/src/ux/` | `dimension.rs` (Pass 1 ordered decision list), `plan.rs` (`build_ux_plan`, `column_order`, `ids` testid consts), `diagnostics.rs`, `sort.rs` (`resolve_ux_sort_plan`) |
| `crates/codegraph-generate/src/ui/page.rs` | `resolve_ux_context` — plan → list-page context; `crates/codegraph-generate/templates/ui/{list_page,_ux_cell,list_timeline,child_section}.tera` render it |
| `crates/codegraph-generate/src/ifml/route_generator/` | `resolve_column_ux` (lookup tier > rules > pack > inference), `resolve_generation_ux`, `TableLayout::Timeline`, event tiering; `crates/codegraph-generate/templates/ifml/svelte/page.tera` |
| `crates/codegraph-generate/src/api/handler.rs` + `ddd/repository_emitter/query_search.rs` | `?sort=`/`?order=` validation (allow-list 400s) + quoted ORDER BY with `, id ASC` tiebreaker |
| `crates/codegraph-generate/src/ui/e2e_test/` (11 families + `pom_ctx.rs`), `crates/codegraph-generate/src/ifml/e2e_test/` (`ux_plans.rs`, spec assembly, `pom.rs`/`pom_render.rs`/`kernel.rs`) | `{seg}.ux.test.ts` / `{view}.ux.spec.ts` emitters (per-feature gated blocks) + the POM emitters (below) |

Flag plumbing: `ux_rules = true` under `[features]` in profiles.toml
(default/ui/fullstack/ci ON; non-bool is a hard error) →
`BuildPlan.ux_rules`; `--ux-rules <file>` CLI on generate/run/ifml-generate
wins over the profile flag (missing file = hard error; plan-less runs
still honor it); resolved into `GeneratorOpts.ux_rules` +
`ProjectConfig.ux` (templates read `project.ux.*`). `codegraph init`
scaffolds `ux_rules = true`. Selector semantics: first-match-wins,
project-ahead-of-pack; `dimension` is payload (not selector) on rules
carrying a classification/pg_type/name_pattern selector, and an override
cancels the column's money hint.

Byte-identity contract: flag off ⇒ no plan, no ux context keys, no sort
surface, no ux spec files — pinned by committed pre-feature tree snapshots
(`UX_RULES_BLESS=1` rebless, rev/path/hex normalization). Since the POM
(#316/#317) the snapshots INCLUDE the test-infra files (kernel + page
classes) — flag-off output is unchanged *modulo* those reviewed files;
every rebless carries an enumerated diff. Gates:

```bash
cargo test -p codegraph --test ux_rules_tests                 # 18, node-free PR-CI net
cargo test -p codegraph --test ux_rules_byte_identity_tests   # 4, the canary
cargo test -p codegraph --test ui_e2e_test_tests              # 23, ux spec gating
cargo test -p codegraph --test ifml_codegen_gate -- --ignored --nocapture  # nightly, ux-ON fixture + `ux` category
```

POM (Playwright Object Model, #315–#318): every generated spec drives the
UI through an emitted page-object layer — zero raw testids in spec
bodies. Entity side (`ui-e2e-test` generator): kernel
`ui/tests/generated/_support/pom.ts` (from `templates/ui/test/_pom_kernel.tera`,
emitted once per run by the first spec-emitting entity) + per-entity
`ui/tests/generated/{domain}/{seg}.page.ts` (from `templates/ui/test/page.tera`),
imported by all 11 `{seg}.{family}.test.ts` families. IFML side
(`ifml-e2e-test` generator): Rust-built kernel
`tests/pages/support/{base-page,ux-table}.ts` (`ifml/e2e_test/kernel.rs`,
surface parity with the entity kernel pinned by a unit test) + per-view
`tests/pages/{view-kebab}-page.ts` (`ifml/e2e_test/pom.rs` plan →
`pom_render.rs`), imported by every `{view}.spec.ts`/`.workflow.spec.ts`/
`.ux.spec.ts`. Emission is **spec-infra gated, never ux-flag gated**
(entity: any-op entities; IFML: any view with payloads); removed
views'/entities' page classes are swept by the next regeneration (T4 pin).
The kernel contract (`BasePage`/`UxTable`: chips/sort/overflow
menu/copy/timeline/Intl mirrors/zebra-inactive) is the test-object
lowering of the ux plane; IFML page classes build from the same
resolution the route generator renders from (`ComponentSelectors`,
`workflow_for_entity`, `html_input_for_dsl` typed fills). IFML POM specs
run live in the nightly gate (T3); the entity POM is pin/snapshot-verified
plus a typecheck over the regenerated review fixture
(`grafeo_e2e_tests -- grafeo_candidate_inspect_output`, then tsc over
`ui/tests/generated/`) — live entity-app e2e runs belong to consumer
projects. Canonical testid table + POM surface map: `docs/ux-rules.md`
§5–§6.

Adding a new rule: (1) dimension/display vocabulary if needed
(`codegraph-config/src/ux/`, closed kebab set), (2) pack default in
`ux_default.toml`, (3) Pass-1 inference branch or rule selector/payload in
`codegraph-generate/src/ux/`, (4) BOTH emitters (entity templates + IFML
`page.tera`), every new block ux-gated so flag-off stays byte-identical,
(5) `{seg}.ux.test.ts` assertion + Rust pin in `ux_rules_tests.rs`,
(6) new ux capability ⇒ consider its POM surface + spec assertion (a
`UxTable` kernel method and/or a page-class section — specs assert
through the page objects, never raw testids),
(7) gate category in `ifml_codegen_gate.rs`.

## gRPC Code Generation

### Overview

Four gRPC generators produce `.proto` files and tonic-based Rust server code alongside the existing REST API. JSON Schema drives the data model; gRPC generators read the same Grafeo graph as the REST generators.

### Generators

| Generator | Kind | Output |
|-----------|------|--------|
| `grpc_proto` | Entity | `proto/{domain}/{module}.proto` — messages + service definition |
| `grpc_service` | Entity | `src/api/grpc/{module}_grpc.rs` — tonic server impl + `From` conversions |
| `grpc_router` | Domain | `src/api/grpc/{domain}_router.rs` — service registration |
| `grpc_scaffold` | Global | `proto/shared.proto`, `src/api/grpc/mod.rs`, shared conversion helpers |

### Architecture layers

| Layer | Location | Notes |
|-------|----------|-------|
| **Type mapping** | `crates/codegraph-generate/src/grpc/proto_type.rs` | Maps `RefClassificationKind` → proto/tonic types. 34 unit tests |
| **Proto context** | `crates/codegraph-generate/src/grpc/proto_context.rs` | Queries graph, builds messages (entity + CRUD + search + tree + transition) |
| **Proto generator** | `crates/codegraph-generate/src/grpc/proto.rs` | `GrpcProtoGenerator` — renders `proto_message.tera` + `proto_service.tera` |
| **Service generator** | `crates/codegraph-generate/src/grpc/service.rs` | `GrpcServiceGenerator` — renders `server_impl.tera` + `conversions.tera` |
| **Router generator** | `crates/codegraph-generate/src/grpc/router.rs` | `GrpcRouterGenerator` — renders `domain_router.tera` |
| **Scaffold generator** | `crates/codegraph-generate/src/grpc/scaffold.rs` | `GrpcScaffoldGenerator` — shared proto + `mod.rs` + conversion helpers |
| **Templates** | `crates/codegraph-generate/templates/grpc/` | 6 Tera templates (proto, service, shared, conversions, server impl, router) |
| **Build integration** | `crates/codegraph-generate/templates/scaffold/build_rs.tera` | Conditional proto compilation via `tonic_build`. Generates both server AND client code |
| **Profile control** | `profiles.toml` | `grpc_backend = true` feature gates the 4 generators |

### Field numbering strategy

- `id` = field number 1
- Entity properties = sequential field numbers starting at 2
- `created_at` = 998, `updated_at` = 999 (synthetic timestamps)

### Codelist enum threshold

- `InlineEnum` → proto `enum`
- `CodelistReference` with ≤20 values → proto `enum`
- `CodelistReference` with >20 values → proto `string`

### Proto compilation

The generated `build.rs` walks the `proto/` directory tree and compiles all `.proto` files via `tonic_build`:

```rust
tonic_build::configure()
    .build_server(true)
    .build_client(true)
    .compile(&protos, &["proto"])
```

Setting `build_client(true)` causes tonic to auto-generate typed client structs (`{Entity}ServiceClient<T>`) — zero additional codegen needed.

### Dependency graph

```
ProtoContext (context builder)
    │
    ▼
proto_type_from_field() (type mapping)
    │
    ▼
GrpcProtoGenerator → .proto files (messages + service)
    │
    ▼
GrpcServiceGenerator → .rs files (server impl + conversions)
    │
    ▼
GrpcRouterGenerator → domain router (service registration)
    │
    ▼
GrpcScaffoldGenerator → shared.proto + mod.rs + convert.rs
    │
    ▼
ScaffoldGenerator integration → build.rs + Cargo.toml (has_grpc flag)
```

## Test Framework

A composable, output-type-agnostic test harness lives at `crates/codegraph/tests/test_framework/`.

### OutputValidator trait

```rust
pub trait OutputValidator: Send + Sync {
    fn name(&self) -> &str;
    fn validate(&self, files: &[GeneratedFile], work_dir: &Path) -> Result<(), Vec<String>>;
}
```

### Built-in validators

| Validator | Checks | Reusable for |
|-----------|--------|-------------|
| `SnapshotCollector` | Collects files into a map for manual assertion | All generators |
| `FilePresenceValidator` | Required files exist | All generators |
| `StringPatternValidator` | Content contains/avoids patterns | All generators |
| `ProtoCompileValidator` | `protoc` compilation (skipped if absent) | Proto output |

### Usage

```rust
#[path = "test_framework/mod.rs"]
mod test_framework;

let test = GeneratorTest {
    db: &engine,
    config: &config,
    tera: &tera,
    output_dir: temp_dir.path(),
    validators: vec![
        Box::new(FilePresenceValidator::new("proto_check", vec!["proto/recruiting/candidate.proto".into()])),
    ],
};
let files = test.run().expect("generation failed");
```

## VS Code Extension

### Location & Structure

```
codegraph-vscode/
├── package.json                    # Extension manifest
├── src/
│   ├── extension.ts                # Activation entry point
│   ├── commands/register.ts        # 4 commands
│   ├── lsp/client.ts               # LSP client (dynamic import)
│   ├── server-manager.ts           # Process lifecycle
│   ├── completion/providers.ts     # Completion provider
│   ├── status-bar.ts               # LSP status indicator
│   └── webview/
│       ├── panel.ts                # WebView panel manager
│       ├── parser.ts               # Lightweight JS IFML parser
│       └── sync.ts                 # Model types + sync protocol
├── webview/                        # SvelteFlow diagram app
│   ├── package.json                # Svelte, @xyflow/svelte, Vite
│   ├── vite.config.ts              # IIFE build → dist/webview/
│   └── src/
│       ├── App.svelte              # Main SvelteFlow canvas
│       ├── main.ts                 # mount(App, #root)
│       ├── types.ts                # IFML model types
│       ├── sync.ts                 # SyncClient (acquireVsCodeApi)
│       ├── nodes/                  # Custom node components
│       │   ├── ViewContainerNode.svelte
│       │   ├── ViewComponentNode.svelte
│       │   ├── EventNode.svelte
│       │   └── ActionNode.svelte
│       ├── edges/
│       │   ├── NavigationFlowEdge.svelte
│       │   └── DataFlowEdge.svelte
│       ├── palette/Palette.svelte  # Element toolbox
│       └── property-sheet/PropertySheet.svelte
├── grammar/                        # Tree-sitter grammar for IFML
│   ├── grammar.js                  # 54 grammar rules
│   └── queries/                    # SCSS queries
├── syntaxes/                       # TextMate grammar fallback
├── test/                           # VS Code extension tests
└── dist/webview/                   # Built SvelteFlow bundle
```

### Key VS Code extension facts

- **Import caveat**: `vscode-languageclient` uses dynamic `import()` to avoid
  `require()` failure in the packaged VSIX (which excludes `node_modules/`).
  `LspClient` is imported via `await import('./lsp/client')` in `extension.ts`.
- **CSP**: The WebView HTML uses `default-src 'none'; style-src <cspSource> 'unsafe-inline';
  script-src 'nonce-<nonce>' 'unsafe-eval'; img-src <cspSource> data:;`
- **Mount target**: `main.ts` mounts to `document.getElementById('root')!`.
- **Message flow**: WebView sends `sync/ready` on load → extension sends
  `sync/modelUpdate` with parsed IFML model.
- **SvelteFlow**: v1.5 uses named exports (`{ SvelteFlow }` not default).
  Requires `bind:nodes` / `bind:edges` for Svelte 5 two-way binding.
- **Vite build**: Uses `define: { 'process.env': {} }` to fix `process is not defined`
  error from `@xyflow/svelte` dependencies.
- **@xyflow/svelte**: ^1.5.2, Svelte 5.56.0, Vite 6

### Build & install

```bash
cd codegraph-vscode
npm run build:webview      # builds SvelteFlow → dist/webview/
npm run compile             # compiles TypeScript → out/
npx vsce package            # creates .vsix
code --install-extension codegraph-ifml-0.1.0.vsix --force
# Reload VS Code completely
```

### Testing

```bash
npm run test:compile        # compiles test files → out/test/
npx tsx test/run.ts         # runs VS Code extension tests
# Or from development path:
npx tsx test/run-vsix.ts    # tests against installed VSIX
```

Tests verify: extension activation, command registration, ifml language ID,
`.ifml` file recognition. 4 tests, all passing.

## VS Code Commands

| Keybinding | Command | When |
|------------|---------|------|
| `Ctrl+Shift+I` | `ifml.openDiagram` | Any editor (shows error if not .ifml) |

Commands: `ifml.openDiagram`, `ifml.validate`, `ifml.generate`, `ifml.refreshLsp`

## LSP Server

### Location

`crates/codegraph/src/lsp/` — Rust binary `codegraph lsp`

```bash
cargo run -- lsp --schemas schemas/ --classifier classifier.toml --config domains.toml
```

### Test coverage

- 5 LSP server tests (initialize, diagnostics, completions, notification)
- Tests use `lsp_server::Connection::memory()` + `tokio::spawn`

## Testing

```bash
# Rust tests
cargo test --workspace                    # all tests (969+)
cargo test -p codegraph -- lsp            # 26 LSP server tests
cargo test -p codegraph --test ifml_e2e_tests  # 8 E2E tests
cargo test -p codegraph-generate --lib -- ifml  # 34 IFML generator tests
cargo test -p codegraph --test init_tests # project lifecycle integration tests

# Dialect tests
cargo test -p codegraph-generate --lib -- db::dialect  # 12 dialect unit tests

# gRPC tests (all levels)
cargo test -p codegraph-generate --lib -- grpc     # 34+ unit tests
cargo test -p codegraph --test grpc_snapshot_tests  # Level 2: Insta snapshots
cargo test -p codegraph --test grpc_compile_tests   # Level 3: protoc compilation

# Profile smoke tests (includes gRPC profile validation)
cargo test -p codegraph --test profile_smoke_tests

# Generated integration testing (#463) — node-free PR-CI pins
cargo test -p codegraph --test hurl_contract_tests    # 2, hurl golden + [hurl] manifest coupling
cargo test -p codegraph --test ifml_auth_setup_tests  # 5, auth.setup/personas/auth specs/journeys/POM

# Ops harness tests (codegraph-ops + ops generator)
cargo test -p codegraph-ops            # 238 harness tests (suites, proc, db, migrate, ext, metrics, doctor, bundle, registry, freshness, results)
cargo test -p codegraph --test ops_generator_tests  # 7 tests + 1 ignored compile test (manifest + testkit emission, OpsConfig::load contract)
cargo clippy -p codegraph-ops --all-targets         # must be warning-free

# Ignored integration tests (run in CI's test-ops-integration job with a postgres:15 service)
cargo test -p codegraph-ops --test db_integration -- --ignored --nocapture   # needs DATABASE_URL (default postgres://postgres:postgres@localhost:5432/postgres)
cargo test -p codegraph --test ops_generator_tests -- --ignored --nocapture  # slow: compiles the emitted testkit crate
cargo test -p codegraph --test review_api_suite -- --ignored --nocapture     # live acceptance gate: fixture regen + testkit api + pool mode + authz over HTTP

# Full pipeline integration (requires protoc)
cargo test -p codegraph --test grafeo_e2e_tests -- grafeo_all_entity_generators_produce_output_for_candidate

# VS Code extension tests
cd codegraph-vscode
npm run test:compile
npx tsx test/run.ts

# E2E pipeline
cargo run -- run --schemas /tmp/ifml-e2e/schemas \
  --classifier /tmp/ifml-e2e/classifier.toml \
  --config /tmp/ifml-e2e/domains.toml \
  --ifml-files /tmp/ifml-e2e/app.ifml \
  --output /tmp/ifml-e2e/output
```

## Pipeline commands

```bash
# Full pipeline: ingest + classify + generate
cargo run -- run --schemas <dir> --classifier classifier.toml \
  --config domains.toml --output <dir>

# With IFML DSL files
cargo run -- run --schemas <dir> --classifier classifier.toml \
  --config domains.toml --ifml-files app.ifml --output <dir>

# Classify only (show entity/VO decisions)
cargo run -- classify --schemas <dir> --classifier classifier.toml \
  --config domains.toml

# Scaffold a new consumer project (interactive when NAME omitted)
cargo run -- init my-app --codegraph-path ~/git/codegraph

# Validate an existing consumer project
cargo run -- doctor --config domains.toml --schemas schemas \
  --classifier classifier.toml --profiles-config profiles.toml

# Grow an existing project with a new domain
cargo run -- add domain billing
```

## Project Initialization (init / doctor / add domain)

### Overview

`codegraph init [NAME]` scaffolds a consumer monorepo. `codegraph doctor`
validates the result, and `codegraph add domain <name>` grows it. Init is
**mox-first**: the scaffold emits `model/<domain>.mox` per domain and NO
`schemas/` directory or `classifier.toml` (JSON interop is documented via
`import schema` / `codegraph migrate`). The scaffold is generated from 15
Tera templates in `crates/codegraph-generate/templates/project/` (see
"Templates & context" below).

### Scaffolded file tree

15 outputs by default (`--domains common`): 14 fixed files + one
`model/<domain>.mox` per domain.

| File | Purpose |
|------|---------|
| `Cargo.toml` | Workspace: members `{name}-graph` + `ops/testkit`; codegraph crates as `git+rev` deps (or `path` deps with `--codegraph-path`); `exclude = ["generated"]` |
| `{name}-graph/Cargo.toml`, `{name}-graph/src/main.rs` | Wrapper binary: clap `Run`/`Classify`/`Generate`/`Doctor` calling `codegraph::driver`; `Run`/`Classify` take repeatable `--mox-files`, `--schemas`/`--classifier` are optional with no defaults |
| `model/{domain}.mox` | Starter mox model per domain (`{Domain}TodoListType` + `{Domain}TodoItemType`, `refers`-linked — the capitalized domain prefix keeps class names unique across packages, since multi-package models compile against ONE union namespace where bare names must be unambiguous); the primary model source. With `--rosetta`: `model/{domain}.rosetta` starters (namespace `{app_name}.{domain}`, one `<Pascal>Type` + `<Pascal>Status` enum) instead — no `.mox` |
| `domains.toml` | One entry per domain (label, schema_dir, postgres_schema); no `entities` key — mox is author-declarative |
| `profiles.toml` | Profile meta (`name`/`version`/`app_name`, `domain_types_base`) + feature flags (`ops_backend`, `grpc_backend`, `ifml_backend`, `has_admin_cli`, `database_target`, `persistence_provider`, `deployment_topology`) |
| `extension-points.toml` | Extension points config |
| `codegraph-ops.toml` | Seeded ops manifest with `mox_files = ["model/<d>.mox", ...]` (no `schemas_dir`/`classifier` keys; see "Ops Harness" section). With `--rosetta`: `rosetta_files = ["model/<d>.rosetta", ...]` instead |
| `ops/testkit/Cargo.toml`, `ops/testkit/src/main.rs` | Testkit workspace member |
| `hurl/health.hurl` | Health-check hurl file |
| `justfile` | Recipes: `generate`/`classify`/`doctor` (all pass `--mox-files model/<d>.mox` per domain), `api`, `e2e`, `full`, `clean`. With `--rosetta`: recipes pass `--rosetta-files model/<d>.rosetta` instead |
| `.gitignore` | Ignores `generated/` |
| `README.md` | Getting-started readme (mox-first quickstart + layout) |
| `.github/workflows/ci.yml` | CI workflow (generate job runs mox-first) |

### Layout decisions

- **Rev pinning**: `crates/codegraph/build.rs` embeds the checkout's git rev
  at build time (`cargo:rustc-env=CODEGRAPH_GIT_REV`, exposed via
  `codegraph::rev::codegraph_rev()`). `init` uses it as the default `rev` for
  the workspace's codegraph deps, so the scaffold pins the exact codegraph
  revision that generated it. `--rev <sha>` overrides.
- **`--codegraph-path <dir>`**: switches all codegraph deps to local path
  deps (`{dir}/crates/...`) instead of `git+rev`, for local development.
- **`generated/`**: all generator output lands there; it is excluded from the
  workspace and gitignored. The wrapper binary and config stay in the repo.
- **Safety**: init refuses to overwrite existing files unless `--force`, and
  a path containment guard keeps writes inside the project dir.

### Subcommand reference

#### `codegraph init [NAME]`

`NAME` prompts interactively when omitted.

| Flag | Default | Meaning |
|------|---------|---------|
| `--output <dir>` | `./{name}` | Parent dir to create the project in |
| `--domains a,b` | `common` | Comma-separated domain names |
| `--database-target` | `postgres` | DB dialect (`postgres`/`sqlite`) |
| `--persistence-provider` | `sea_orm` | `sea_orm`/`cornucopia` |
| `--deployment-topology` | `monolith` | `monolith`/`workers` |
| `--grpc`, `--ifml` | off | Enable gRPC / IFML features in `profiles.toml` |
| `--rosetta` | off | Rosetta-first scaffold: `model/<domain>.rosetta` starters (sigil parse+lower+resolve-verified before write) instead of `.mox`, `rosetta_backend = true` in `profiles.toml`, `rosetta_files` in the ops manifest, `--rosetta-files` justfile recipes. Rosetta types are auto-scored (no `entities` key either) |
| `--no-ops` | off | Disable the ops generator/profile feature (testkit member still scaffolded) |
| `--rev <sha>` | embedded rev | Codegraph git rev to pin |
| `--codegraph-path <dir>` | none | Path deps to a local codegraph checkout |
| `--force` | off | Overwrite existing files |
| `--template-dir <dir>` | repeatable | Additional template dirs (later take precedence) |

#### `codegraph doctor`

| Flag | Default | Checks |
|------|---------|--------|
| `--config` | `domains.toml` | domains.toml parses |
| `--schemas` | optional | schemas dir contains JSON schema(s); absent + no `--mox-files` = hard failure, absent + mox files = info line (mox-first shape) |
| `--classifier` | optional | classifier.toml parses; absent + JSON schemas present = hard failure, absent + no JSON schemas = info line |
| `--profiles-config` | optional | profiles.toml parses + BuildPlan capability validation |
| `--rosetta-files <file>` | repeatable | Each file sigil-verified (parse → lower → resolve; severity-Error diagnostics = hard failure). A namespace whose last segment matches no domains.toml key = WARN (generation silently drops it); `import <ns>.*` with no file among `--rosetta-files` and no matching domain key = WARN. Prints an INFO line with the embedded sigil rev (`rev::sigil_rev()`, WARN when unpinned) |

Doctor's model-source matrix (zero warnings is the intentional mox-first
new-project shape): schemas dir absent + mox files → INFO; schemas dir
present but empty + mox files → WARN (misconfiguration); schemas dir with
JSON → PASS; no mox files and no schemas → hard failure. `check_mox_files`
accepts multiple `--mox-files` (one per domain), validates `import schema`
targets per file (paths resolve relative to each file), and compiles the
WHOLE set as ONE union workspace — the `run` pipeline's semantics, where
every file lowers against the combined namespace of all packages. A
per-file compile cannot resolve cross-package type references (false
`unknown type` failures) and cannot see ambiguous bare names (silent
zero-class ingestion) — doctor must match generation. Diagnostics are
attributed per file: an Error-severity diagnostic fails that file; each
compiling file's package is line-scanned and checked against domains.toml.

Hard failures (non-zero exit): domains.toml, classifier.toml (when JSON
schemas are present), profiles.toml, schemas dir (when no mox files),
codegraph-ops.toml (`OpsConfig::load`). Warnings only: missing
profiles.toml / codegraph-ops.toml, empty schemas dir in mox mode,
Cargo.toml rev pins vs the binary's embedded rev (mismatch WARN; local
path deps PASS as development mode), missing `psql`/`npx`/`hurl` tools.

#### `codegraph add domain <name>`

Appends a `[domains.<name>]` entry (label, schema_dir, postgres_schema) to
`domains.toml` and creates a starter model from the shared starter
template (`init/model_starter.rs`), verified before write. Rosetta-first
projects (`model/*.rosetta` present or `--rosetta`) get
`model/<name>.rosetta` namespaced `{app_name}.{domain}`, sigil-verified
(parse → lower → resolve); everything else gets `model/<name>.mox`
compile-verified with the rex compiler. No `schemas/<name>/` directory is
created. Rejects duplicate domain names.

### Hello-world TODO example

The scaffold ships a working TODO example (two entities per domain:
`{Domain}TodoListType` + `{Domain}TodoItemType`, linked by `refers` —
single-domain projects render `CommonTodoListType`) instead of a
placeholder model. The
mox-first generate lifecycle is verified end-to-end by
`init_scaffold_runs_mox_first` (scaffold → `driver::run` with ONLY
`--mox-files` + config → DDL contains `todo_list` + `todo_item` with the
refers-derived FK):

```
codegraph init todo-app
cd todo-app
just generate          # 198 files, 0 errors, 0 warnings (mox-first)
cargo build --manifest-path generated/Cargo.toml
just api               # ops api suite gate: migrate, hurl CRUD smoke, RLS, graceful shutdown
```

`just generate` numbers verified with the real pipeline (198 files);
the `just api` gate is verified by the ops suite (needs a Postgres) rather
than a fixed spec count here. The generator stubs below remain load-bearing
for fresh projects: `errors` generator always emits per-domain
`errors.rs` (InternalError-only when no definitions), codelist generators
emit empty `src/codelist/mod.rs` (both app and domain-types crates),
`domain_types_scaffold` emits generic `context.rs`/`query.rs`/`codelist`,
scaffold Cargo.toml declares `thiserror`, the pid file name uses the app
name (not hardcoded `hr-app`), and `0000_extensions.sql` installs pgcrypto.

### Lifecycle walkthrough

```bash
codegraph init my-app                       # scaffold (add --codegraph-path ~/git/codegraph for local dev)
cd my-app
just doctor                                 # validates config + toolchain
just generate                               # wrapper run -> generated/ (excluded from workspace)
just api                                    # ops testkit api suite (preflight, migrate, hurl, curl, RLS)
just e2e                                    # ops testkit e2e (Supabase -> generate -> migrate -> build -> Playwright)
just full                                   # api then e2e
```

### Templates & context

Project templates live in `crates/codegraph-generate/templates/project/`
(15 templates, embedded via that crate's build.rs, shadowable via
`--template-dir`). The render context is `ProjectTemplateContext` in
`crates/codegraph/src/init/context.rs`, and the canonical (template,
output-path) list is `PROJECT_TEMPLATES` in the same file; output paths
support `{graph}` and `{domain}` placeholders. `project/model_mox.tera` is
special-cased: it renders once per domain with `domain`/`domain_label`
inserted into the context (one `model/<domain>.mox` per domain).
`crates/codegraph/src/init/model_starter.rs` renders the same template for
`add domain`.

Adding a new template:

1. Add `project/<name>.tera` under `crates/codegraph-generate/templates/project/`.
2. Append its `(template, output)` pair to `PROJECT_TEMPLATES` in
   `crates/codegraph/src/init/context.rs`.
3. Add any new fields to `ProjectTemplateContext` (it serializes into the
   Tera context).
4. Update the `init` tests / `file_tree()` expectations if the layout changed.

## Consumer Performance & Persisted-Graph Cache (issue #275)

Measured on the crewbase consumer loop (9 mox packages, ~38 entities,
~170 properties — the largest known consumer model):

- **Cold generate** (no cache): ingestion dominates — ~44 min. Ingest
  fans out one MATCH-per-edge with no label/property index, so edge
  inserts scan the whole node store: O(nodes × edges) in Grafeo. The
  in-memory engine carries no query timeout; the PERSISTENT path
  (`Config::persistent`) defaults to a 30s per-query timeout that
  mid-size edge inserts cross — `GrafeoEngine::persistent` therefore
  calls `.without_query_timeout()`. Revisit when Grafeo grows indexes;
  the real fix is batching edge inserts or indexing the MATCH.
- **Warm generate** (`run_with_graph_cache(args, Some(cache_dir))`, the
  consumer-wrapper pattern): an inputs hash over every model/config
  input + the graph format version is stored beside the checkpoint; on
  a match the persisted graph reopens and ingestion is SKIPPED —
  generation alone runs in ~1 min.
- **Poisoned-cache gotcha**: an aborted persistent ingest (timeout,
  crash) leaves committed WAL from the partial run in `graph.grafeo`.
  The next run misses the (unwritten) hash, reopens the SAME file, and
  re-ingests on top of the partial graph — duplicated nodes and
  silently missing projections (junction tables vanish, migration
  counts drop). There is no duplicate detection: after ANY failed
  persistent-ingest run, delete the cache directory before rerunning.
- **Generator-only changes don't invalidate the hash** (inputs are the
  model/config files): when iterating on generators against a consumer
  model, a warm cache makes each edit→generate→check cycle ~1 min.

## Template Overrides

### The `--template-dir` flag

Available on `generate`, `run`, and `init` commands. May be specified multiple times; later directories take precedence.

```
Paths to additional template directories. Templates in these directories
shadow codegraph's built-in templates by name. May be specified multiple
times; later directories take precedence.
```

### How template shadowing works

Implemented in `crates/codegraph-generate/src/template_engine.rs`:

1. **`create_tera_with_overrides()`** loads the built-in templates embedded via that crate's build.rs first
2. It then iterates override directories in order, calling `merge_tera_dir()` for each
3. **`merge_tera_dir()`** walks each directory, reading `.tera` files and registering them by their relative path name
4. A template with the same relative path from a later directory **shadows** the earlier one — no merging, full replacement

### Available Tera custom filters

| Filter | Description |
|--------|-------------|
| `snake_case` | Converts a string to `snake_case` |
| `upper_camel` | Converts to UpperCamelCase (strips trailing `Type` suffix first) |
| `pascal_case` | Converts to PascalCase |
| `kebab_case` | Converts to `kebab-case` |
| `pluralize` | Pluralizes a word (simple rules: `s`/`es`/`ies`) |
| `truncate_pg` | Truncates to PostgreSQL max identifier length (63 chars) |
| `dollar_quote` | Wraps a string in single quotes with proper escaping |
| `strip_pg_quotes` | Removes double-quote characters from PostgreSQL identifiers |
| `quote_pg` | Double-quotes a PostgreSQL identifier if it is a reserved word |

### Example: overriding SQLite templates

```bash
# Override the SQLite table template with a custom version
cargo run -- run --schemas schemas/ --classifier classifier.toml \
  --config domains.toml --output out/ \
  --template-dir ./my-overrides/

# Multiple override directories; later ones win
cargo run -- generate --config domains.toml --output out/ \
  --template-dir ./team-templates/ --template-dir ./local-tweaks/
```

Place a `.tera` file at the matching relative path to shadow it. For example, `my-overrides/db/sqlite/table.tera` shadows `crates/codegraph-generate/templates/db/sqlite/table.tera`.

## Database Dialect Support (feat/sqlite-support)

### Overview

Codegraph supports configurable database target dialects via the `SqlDialect`
trait. Currently two dialects are implemented:

| Dialect | `database_target` value | Key features |
|---------|------------------------|--------------|
| PostgreSQL | `"postgres"` (default) | UUID, JSONB, TIMESTAMPTZ, RLS, extensions, PL/pgSQL, schemas |
| SQLite | `"sqlite"` | TEXT, INTEGER, no RLS, inline triggers, FTS5, STRICT tables |

### Architecture

```
profiles.toml                           database_target from features
    │                                           │
    ▼                                           ▼
BuildPlan                             ───►   ProjectConfig.database.database_target
    │                                           │
    ▼                                           ▼
DB Generators (ddl, entity, etc.)     ───►   SqlDialect trait
    │                                           │
    ▼                                           ▼
Tera templates                              {{ project.database_target }}
templates/db/sqlite/*.tera               (available in all template contexts)
```

### SqlDialect trait

Defined at `crates/codegraph-generate/src/db/dialect.rs`:

- **30+ methods** covering: type mapping, default expressions, feature flags,
  identifier handling, trigger syntax, FTS engine selection, plus
  `validate_column_type` (per-dialect column-type legality)
- `DatabaseTarget` enum: `Postgres`, `Sqlite` (default: `Postgres`)
- Factory: `dialect_for_target(DatabaseTarget)` returns `Box<dyn SqlDialect>`
- 14 unit tests

### SQLite STRICT validation + parse gate

Two generation-time gates keep invalid SQLite DDL from ever reaching a
migration file (previously `DATE`, `NUMERIC(10,2)`, and `BYTEA` columns
passed through raw into STRICT tables and failed at `sqlite3` apply time):

1. **Semantic type check** (`dialect.rs` `SqliteDialect::validate_column_type`,
   wired into `apply_dialect_type_mapping` in `db/ddl/mod.rs`): every column type
   must either map via `map_pg_type` or be a native STRICT type
   (`INT`/`INTEGER`/`REAL`/`TEXT`/`BLOB`/`ANY`). `DATE` → `TEXT` (ISO-8601,
   lossless), `BYTEA` → `BLOB`, precision-suffixed numerics → `REAL`. Range
   types (`DATERANGE`, ...) and array types (`TEXT[]`, ...) are deliberately
   unmapped and REFUSED — coercing them to TEXT would silently lose their
   operators. Unrepresentable types are a hard generation error naming the
   table and column.
2. **Parse gate** (`db/sqlite_gate.rs`, sqlglot-rust `=0.10.30`): every
   generated SQLite statement must parse under the SQLite dialect before the
   file is emitted. Trigger bodies (`CREATE TRIGGER ... BEGIN ... END;`) are
   skipped textually (sqlglot-rust has no trigger grammar); everything else
   (tables, indexes, FTS5 virtual tables, views) must parse. Postgres output
   is NOT gated — it contains PL/pgSQL the generic parser rejects. Known
   limitation: a template regression inside a trigger body is not caught.

### Profile configuration

```toml
[profiles.default.features]
database_target = "sqlite"     # default is "postgres"
```

The `database_target` value is parsed from the `[features]` table in
`profiles.toml` and stored in `BuildPlan.database_target`. It's propagated
to all templates via `ProjectConfig.database.database_target`.

### SQLite templates

Located at `crates/codegraph-generate/templates/db/sqlite/`:

| Template | Purpose |
|----------|---------|
| `table.tera` | CREATE TABLE with STRICT mode, TEXT types |
| `entity.tera` | SeaORM entity without `schema_name` attribute |
| `trigger.tera` | Inline CREATE TRIGGER (no PL/pgSQL) |
| `fts.tera` | FTS5 virtual table with sync triggers |
| `codelist.tera` | INSERT OR IGNORE for idempotent seed |
| `rls.tera` | Placeholder (SQLite has no RLS) |
| `domain_event_trigger.tera` | Simple event table insert (replaces pgmq) |

Generators select the template directory based on the dialect. The existing
`templates/db/` templates remain the PostgreSQL originals and are untouched.

### Adding a new dialect

1. Add a variant to `DatabaseTarget` in `dialect.rs`
2. Implement `SqlDialect` for the new target
3. Add templates under `templates/db/<target>/`
4. Register the dialect in `dialect_for_target()`
5. Unit tests in `dialect.rs` `#[cfg(test)]` block

## Ops Harness (codegraph-ops + `ops` generator)

### Overview

`crates/codegraph-ops` is a Rust test & deploy harness for codegraph-generated
apps — a re-imagining of the hand-written bash suite (`test.sh`,
`lib/common.sh`, `lib/migrate.sh`, `deploy/smoke-test.sh`,
`scripts/quality-check.sh`) that hr-specs used to maintain (hr-specs has been
ported onto the harness; its bash suite is deleted). It is
configuration-driven and extension-pluggable so every codegraph consumer
shares the same harness while keeping their project-specifics (Xero/Stripe/IRD
integrations, UI-sync rsync steps, integration migrations) as manifest hooks
and extensions.

### Architecture layers

| Layer | Location | Notes |
|-------|----------|-------|
| **Manifest types** | `crates/codegraph-config/src/ops_manifest.rs` | `OpsManifest` (serde TOML): app name, servers/ports, db targets, supabase, capabilities, hurl, hooks, extensions, smoke entity, api version |
| **Harness crate** | `crates/codegraph-ops/` | Runtime: `cli.rs` (clap), `config.rs` (`OpsConfig` resolution), `proc.rs` (SIGTERM→SIGKILL supervision, `Supervisor`, `run_streaming` labeled streaming), `db.rs` (psql wrapper, extension validation), `migrate.rs` (phased migrations, supabase symlinks), `suites/*` (api, cli, ui, e2e, workers, smoke, quality), `ext.rs` (extension protocol + hooks + `HookPolicy`), `metrics.rs` (stage TSV/JSON export), `doctor.rs` (state report + stage-0 fast-doctor subsets), `registry.rs` (`.testkit/services.json` advisory services registry), `freshness.rs` (generator-rev check), `results.rs` (`--results` JSON writers), `bundle.rs` (failure artifact bundles), `pwcache.rs` (Playwright transpile-cache hygiene), `wait.rs`, `env.rs`, `pg.rs` (`PgTarget`) |
| **Generator** | `crates/codegraph-generate/src/ops.rs` | Global generator `ops` — emits `codegraph-ops.toml` + `testkit/` crate into generated output |
| **Templates** | `crates/codegraph-generate/templates/ops/` | `testkit_cargo.tera`, `testkit_main.tera` (shadowable via `--template-dir`) |
| **Profile gating** | `profiles.toml` + `profile/capabilities.rs` | `ops_backend` feature; `cap("ops", Global, Common, &["ops_backend"], &[])` |
| **Contract test** | `crates/codegraph/tests/ops_generator_tests.rs` | Emitted manifest must parse via `OpsConfig::load` (cross-crate) |

### Subcommands (run via the generated testkit binary)

```
cargo run -p testkit -- api        # preflight, migrate, hurl, curl smoke, RLS, shutdown
cargo run -p testkit -- cli        # CLI e2e (starts API first)
cargo run -p testkit -- e2e        # Supabase → generate → migrate → build → Playwright
cargo run -p testkit -- ui         # Playwright only (API must be running)
cargo run -p testkit -- full       # api then e2e (always both; worse exit code wins)
cargo run -p testkit -- workers    # workers topology: regenerate → migrate → build → boot → smoke + hurl
cargo run -p testkit -- smoke      # remote deployment smoke test
cargo run -p testkit -- quality    # cargo test/clippy/fmt + generate + check
cargo run -p testkit -- clean      # stop services, remove generated output (keeps test-results/)
cargo run -p testkit -- clean --deep # also remove test-results/ triage artifacts
cargo run -p testkit -- doctor     # one-shot state report (tree, binaries, DBs, ports, tools, disk)
cargo run -p testkit -- bundle     # assemble a failure artifact bundle on demand
cargo run -p testkit -- ext <name> # run a test extension
cargo run -p testkit -- ext --list # list registered extensions
```

Subcommand flags: `api --no-migrate` / `--rebuild` / `--regen` (stage 11
regeneration validation), `e2e --skip-ui-build` (degrade a failed SvelteKit
production build to a warning; the preview may serve a stale bundle) and
`e2e --retry-failed` (in-session `--last-failed` rerun; a green retry counts
as transient), `clean --deep`, `smoke` flags below. `e2e`/`ui` pass trailing
args through to Playwright (`testkit e2e -- --last-failed`).

Global flags: `--config FILE` (manifest path), `--keep`, `--skip-build`,
`--skip-generate`, `--release`, `--verbose` (unmutes quiet stages —
dependency installs, browser downloads — and echoes each stage's full
captured output; failure tails print regardless), `--metrics FILE` (stage
timings; TSV or JSON via `--metrics-format tsv|json`, default tsv),
`--metrics-format`, `--results FILE` (machine-readable JSON report; api,
e2e, workers), `--retry N` (retry failed hurl files in the api suite up to
N times, default 0), `--pw-retries N` (Playwright `--retries=N` for e2e/ui),
`--allow-gen-errors` (tolerate generation errors instead of failing),
`--allow-gen-rev-mismatch` (downgrade the generator-rev check to a warning),
`--clear-cache` (clear the Playwright transform cache at run start),
`--reuse` (reuse a registry-known service on a needed port instead of
taking it over), `--no-bundle` (skip the automatic failure artifact
bundle), `--codegraph-root PATH` (exported as `CODEGRAPH_ROOT` for hooks
and generated path-dep normalization), `--headed`, `--grep PATTERN`
(repeatable).

When `--config` is absent the manifest is auto-discovered: walk UP from the
cwd looking for `codegraph-ops.toml`, then walk UP from the testkit
executable's directory (cli.rs `find_manifest()`).

Failures print a `hint:` line when the error type has one (missing tools,
config mistakes, port conflicts — `error::hint()` in `cli.rs`).

Manifest values support `{env:VAR}` indirection for `database.*.password`
and `supabase.anon_key`/`service_key`/`jwt_secret` (`config.rs`
`resolve_env()`): unset variables expand to empty; plain strings pass
through unchanged.

`smoke` flags: `--api-url`, `--web-url`, `--expected-commit`,
`--auth-health-url`, `--worker URL` (repeatable for worker pings).

`quality` accepts extra cargo gate names (e.g. `doc`) as trailing args.

Exit codes: 0 success; 1 any harness-reported failure (config error, missing
tool, failed checks or tests); 2 timeout. Usage errors (unknown flags) exit
2 via clap before the harness runs.

### Runbook (`docs/ops-testing.md`)

The full runbook — per-suite stage maps with expected durations at consumer
scale, freshness model, artifact-triage guide, env-var reference, and the
failure-signature → cause → fix playbook (every row from a real hr-specs
validation-run incident) — lives in `docs/ops-testing.md`. Subsystem map
(all shipped on the ops-reliability branch):

- **Streaming/timing** (`proc.rs` `run_streaming`): every long spawn streams
  labeled per-line output (`[build] Compiling …`), stage durations print on
  completion, failure tails (last 50 lines) ALWAYS show, hooks are timed as
  separate `hook <name>` metrics rows; `--metrics` JSON gains per-stage rows
  plus a TOTAL row.
- **Services registry** (`registry.rs`, `--keep`/`--reuse`): every service a
  `--keep` run leaves is recorded in `.testkit/services.json` (advisory —
  corrupt/missing never breaks a run). Port preflight takes over a
  registry-known prior server by default (SIGTERM→SIGKILL on the recorded
  pid) or reuses it with `--reuse` (caveat: it may serve a stale build);
  unknown occupants keep the `fuser -k` error. `clean` kills by registry pid,
  sweeps api/ui + workers ports (gateway 8787 + worker range), removes
  sveltekit logs, and KEEPS `test-results/` unless `--deep`.
- **Freshness** (`freshness.rs`, `preflight.rs`): binary-vs-src mtime check,
  e2e stage-0 pre-check under `--skip-build`, and the generator-rev check —
  `.codegraph-manifest.json` `codegraphCommit` vs the testkit's embedded
  `CODEGRAPH_OPS_REV`; mismatch is a hard error (`--allow-gen-rev-mismatch`
  downgrades; missing manifest warns).
- **Playwright cache hygiene** (`pwcache.rs`, `--clear-cache`): the
  content-hash-addressed transform cache (`/tmp/playwright-transform-cache-*`)
  is cleared wholesale before Playwright (e2e/ui automatically; the flag
  forces it for other suites).
- **Failure bundles** (`bundle.rs`): suites assemble
  `{root}/test-results/artifacts-<ts>/` on failure (`--no-bundle` skips;
  `testkit bundle` on demand) with `logs/`, `hurl/`, Playwright summary
  files, the run's results/metrics, and a `bundle.json` manifest; 200 MB
  soft cap (tail-copy) + keep-3 pruning (`[doctor]`/`[bundle]` manifest
  sections tune doctor disk thresholds and bundle caps).
- **Doctor** (`doctor.rs`): 8 sections (manifest, output tree, binaries,
  databases, ports, extras, tools, disk & caches); stage-0 `run_fast_doctor`
  subsets ("Fast doctor" marker) gate api/e2e before long stages.
- **Results** (`results.rs`): `--results FILE` carries the completed-run
  `ResultsReport` (passed/failed/failures/stages/`hook_failures`/exit);
  failures before the summary stage write the flat `EarlyFailureReport`
  (`suite`/`manifest`/`stage`/`error`/`exit`) instead.

### Extension protocol

- `ext::TestExtension` trait: `name()`, `requires_api_running()`,
  `run(&OpsContext)` — consumers register via `register_extension()` in the
  generated testkit `main.rs` (see `templates/ops/testkit_main.tera`).
- Manifest `[[extensions]]` entries with `exec` run out-of-process via `sh -c`
  (language-agnostic; how hr-specs' Xero/Stripe/IRD-style integrations plug in).
- Manifest `[[hooks]]` entries run at pipeline points: `pre_generate`,
  `post_generate`, `post_migrate`, `pre_e2e`, `post_e2e`, `pre_api`,
  `post_api`, `pre_playwright`.

### Manifest (`codegraph-ops.toml`)

Seeded by the generator from `ProjectConfig`/`BuildPlan` (app name, ports,
db targets, capabilities, api version). Consumers extend:
`database.*.reset_sql`/`seed_sql`, `database.api.grant_role` (default
`app_user`) / `database.api.grant_strict` (default `false`; when `true` the
api suite hard-fails if the grant role is missing DML on any domain table
after migration instead of warning), `supabase` dir + keys,
`hurl.dir`/`skip`/org ids, `smoke.entity` (entity used for the api suite's
curl CRUD checks) + `api_version` (route prefix, default `v1`), `ui_dir`
override (for monorepo sync setups), `[doctor]` (`min_free_gb`, default 2 /
`warn_free_gb`, default 10 — disk headroom gates), `[bundle]` (`max_mb`,
default 200 / `keep`, default 3 — failure-bundle cap and pruning), hooks
(each `[[hooks]]` entry takes `fatal` — missing means fatal, see below),
extensions.

### Consumer integration guide

1. Add `codegraph-ops` to the consumer workspace deps:
   ```toml
   codegraph-ops = { git = "https://github.com/magick93/codegraph.git", rev = "<pinned>" }
   ```
   Pin the same rev as the other codegraph crates. During development use
   `branch = "<branch>"` plus a `[patch."https://github.com/magick93/codegraph.git"]`
   entry pointing `codegraph-ops` at a local path.
2. Enable the generator: `ops_backend = true` under `[features]` in the
   consumer's `profiles.toml` plus `"ops"` in the profile's generator list;
   regenerate. This emits `codegraph-ops.toml` + a `testkit/` crate.
   `smoke.entity` is auto-seeded from the first entity in generation order
   (codegraph-generate `ops.rs`); the codegraph binary stamps `project.codegraph_rev`
   from its own git rev (`main.rs`) so the testkit `Cargo.toml` pins the same
   rev (see `templates/scaffold/cargo_toml.tera`, `templates/ops/testkit_cargo.tera`).
   Consumers on local path deps (no git rev) keep generated pins working with
   a `post_generate` hook that rewrites the generated path-dep lines
   (hr-specs' `repin-codegraph-path-deps` hook is the reference).
3. Edit the manifest: `database.api`/`database.e2e`/`database.e2e_app` targets
   (+ `reset_sql`/`seed_sql`), `supabase` dir + keys, `hurl.dir`/`skip`/org
   ids, `ui_dir` override (monorepo sync setups), and — for e2e generation —
   `graph_binary` + `schemas_dir` + `classifier` + `domain_config`.
4. Run: `cargo run -p testkit -- api` / `e2e` / `full` / `workers` / `smoke` /
   `quality` / `doctor` / `bundle` / `clean` / `ext <name>`. Add `--results`
   and `--metrics` for machine-readable output in CI.
5. Add project-specifics as `[[hooks]]` and `[[extensions]]` (exec-based
   out-of-process entries) or trait-based extensions (register in the
   `testkit_main.tera` registration hook — `register_extension()` before the
   tokio runtime starts). hr-specs dogfooding: pgmq patch as `pre_e2e`,
   crewbase rsync as `pre_playwright`, hr-reports views as `post_migrate`; an
   extensions crate implements `TestExtension` for ird/stripe/xero and the
   testkit workspace member registers them; a justfile delegates to the
   harness; the bash suite is deleted.

   Hook points, suites that fire them, and fatality (#357 — each hook's
   manifest `fatal` flag decides; MISSING means fatal):

   | Hook | Suite(s) | Fires | Fatality |
   |------|----------|-------|----------|
   | `pre_generate` | api + e2e + workers | Before generation (graph binary build may precede) | per-hook `fatal` |
   | `post_generate` | api + e2e + workers | After generation (before the app build) | per-hook `fatal`; workers: warn-only |
   | `pre_e2e` | e2e | After Supabase start, BEFORE migration symlink + `supabase db reset` | per-hook `fatal` |
   | `post_migrate` | api + e2e | After DB reset/migration (api only when migrate=true) | per-hook `fatal` |
   | `pre_playwright` | e2e | After the API is up, BEFORE the SvelteKit production build | per-hook `fatal` |
   | `post_e2e` | e2e | Every e2e path (success and failure) | ALWAYS warn-only (`fatal = true` cannot escalate) |
   | `pre_api` / `post_api` | api | Around the api suite | per-hook `fatal` |

   Each hook is `sh -c "{exec} {args...}"` in the repo root. A failing fatal
   hook aborts the suite with the hook's output tail; a failing non-fatal
   hook warns and its name lands in the results JSON's `hook_failures`.
   Consumers upgrading from pre-#357 manifests: hooks that previously failed
   without aborting (blanket warn-only) now need explicit `fatal = false`.
   Hooks stream under `[hook:<name>]` and are timed as separate metrics rows.
6. CI wiring: run `cargo run -p testkit -- api --metrics ci.tsv` in a job with
   a Postgres service. The codegraph repo's own `test-ops-integration` job
   (postgres:15 service + the `--ignored` integration tests in `.github/workflows/ci.yml`,
   CI triggers on push to `develop` and `master`) is the reference pattern.

### Platform support & prerequisites

Unix-first (Linux/macOS): process supervision is SIGTERM → grace → SIGKILL
(`proc.rs`), `clean` kills ports via `fuser -k`, and consumer hooks commonly
use rsync. Windows is not supported by the process-supervision/port-kill
paths.

Tool prerequisites (validated per-suite; missing tools error or skip):

- `psql` (db access + extension checks), `curl` (health probes, api suite)
- `hurl` (api contract tests — skipped if absent)
- `npx` + Supabase CLI + Docker (e2e Supabase stack)
- `pnpm` + Playwright chromium (ui/e2e; installed via `playwright install chromium`)

### Adding a feature to the harness

1. Module in `crates/codegraph-ops/src/` (or a new suite in `suites/`).
2. Wire the subcommand in `cli.rs` + flag plumbing.
3. Unit tests alongside; `cargo test -p codegraph-ops`.
4. New long-running stages MUST spawn through `proc::run_streaming` /
   `run_streaming_quiet` (never a bare `Command::output()`) so output
   streams under `[label]` and failures carry a tail.
5. Record stage timings with `config.metrics.begin(name)` / `.end()` (hooks
   and nested work: `pause()`/`resume()`).
6. Route failures through the results writers: suites write their
   `ResultsReport` at the summary stage (success AND failure); anything that
   can fail before the summary is covered by the CLI-level early-failure
   writer, which records the current `output::section()` as the stage — so
   keep `output::section` titles meaningful.
7. If the generated manifest needs new seed values, extend
   `OpsManifest` in `codegraph-config` + the generator in codegraph-generate `ops.rs`.

## Persistence Provider System

### Overview

Codegraph supports swappable persistence backends via the `PersistenceProvider`
enum. The DDL generator is always provider-agnostic (it generates SQL, not
ORM code). The entity model and repository implementation are provider-specific
and selected by the `persistence_provider` feature flag.

### Three-layer architecture

```
                         JSON Schema + Policies
                                 │
                                 ▼
                     GraphQuerier (Grafeo graph)
                                 │
                                 ▼
                     build_persistence_entity()
                                 │
                                 ▼
                     PersistenceEntity (IR)        ← ORM-agnostic
                                 │
                    ┌────────────┼────────────┐
                    ▼            ▼            ▼
              SeaOrmBackend  Cornucopia    (future:
              (entity.tera,  Backend       Diesel,
               repo emitter) (.sql files,  SQLx, ...)
                              cornucopia.toml)
```

### PersistenceProvider enum

Defined at `crates/codegraph-generate/src/profile/types.rs`:

| Variant | Config value | Entity model | Repository | Query layer |
|---------|-------------|--------------|------------|-------------|
| `SeaOrm` | `"sea_orm"` (default) | `sea_orm_entity` generator → `#[derive(DeriveEntityModel)]` structs | `repository` + `repository_emitter` → SeaORM ActiveModel/QueryBuilder | `query` generator → SeaORM `EntityTrait::find()` |
| `Cornucopia` | `"cornucopia"` | `cornucopia_queries` generator → `queries/{domain}/{entity}.sql` annotated SQL files | `cornucopia_repo` generator → wrapper around Cornucopia query functions | Cornucopia-generated typed query structs via `bind()/all()/one()/opt()` |

### Profile configuration

```toml
[profiles.default.features]
persistence_provider = "sea_orm"   # "sea_orm" (default) | "cornucopia"

# SeaORM profile — existing generators
[profiles.default.api]
generators = ["ddl", "sea_orm_entity", "dto", "repository", "command", "query", ...]

# Cornucopia profile — alternative generators
[profiles.cornucopia.api]
generators = ["ddl", "cornucopia_queries", "cornucopia_repo", "cornucopia_config", "dto", ...]
```

The `persistence_provider` value is parsed from `[features]` into
`BuildPlan.persistence_provider` and propagated to generators via
`ProjectConfig.database.persistence_provider` (available in all Tera templates).

### PersistenceEntity IR

Defined at `crates/codegraph-core/src/types/persistence.rs`:

| Type | Purpose |
|------|---------|
| `PersistenceEntity` | Top-level ORM-agnostic model: title, table_name, schema_name, rust_type_name, columns, child_tables, relations, policies |
| `PersistenceColumn` | Column descriptor: field_name, column_name, rust_type, pg_type, is_primary_key, is_nullable, is_jsonb, is_range, pg_cast, role |
| `PersistenceColumnRole` | Semantic role: Data, PrimaryKey, TenantScope, SoftDeleteMarker, AuditTimestamp, AuditUser, AuditFlag, ForeignKey, HierarchyParent |
| `PersistenceChildTable` | Child table from a ValueObject property: table_name, struct_name, parent_fk, columns |
| `PersistenceEntityRelation` | Relationship: name, relation_type, related_entity, from/to_column, is_self_ref |
| `PersistencePolicies` | Policy effects translated to ORM-agnostic form: SoftDeleteEffect, TenantIsolationEffect, RowSecurityEffect, AuditEffect, RetentionEffect |

### build_persistence_entity() builder

Defined at `crates/codegraph-generate/src/persistence.rs` — the single source
of truth for extracting entity structure + policies from the graph. Both
`SeaOrmEntityGenerator` and `CornucopiaQueryGenerator` call it.

### Key files

| File | Role |
|------|------|
| `crates/codegraph-core/src/types/persistence.rs` | IR types: PersistenceEntity, PersistenceColumn, policy effects |
| `crates/codegraph-generate/src/profile/types.rs` | `PersistenceProvider` enum + `from_config()` + `BuildPlan` field |
| `crates/codegraph-generate/src/persistence.rs` | `build_persistence_entity()` — graph → IR builder |
| `crates/codegraph-generate/src/db/entity.rs` | `SeaOrmEntityGenerator` — SeaORM model emission (existing, unchanged) |
| `crates/codegraph-generate/src/ddd/repository_emitter/` | SeaORM repository impl emitter (existing, unchanged) |
| `crates/codegraph-generate/src/db/cornucopia_queries.rs` | `CornucopiaQueryGenerator` — annotated SQL file generation |
| `crates/codegraph-generate/src/db/cornucopia_config.rs` | `CornucopiaConfigGenerator` — `cornucopia.toml` with type mappings |
| `crates/codegraph-generate/src/ddd/cornucopia_repo.rs` | `CornucopiaRepoGenerator` — repository adapter wrapper |
| `crates/codegraph-generate/src/project_config.rs` | `ProjectConfig` — Rust fields composed from `#[serde(flatten)]` sub-configs (`identity`/`paths`/`database`/`deployment`/`codegen`/`cargo`/`integration`/`atproto`/`ux`); the serialized Tera map stays flat + generator dispatch in `registry.rs` |
| `profiles.toml` | `persistence_provider` feature flag |

### Policy-aware query generation

Both SeaORM and Cornucopia backends consume the same `PersistencePolicies`
struct built from `PolicyNode` graph data. Policy effects drive:

| Policy | SeaORM effect | Cornucopia SQL effect |
|--------|--------------|----------------------|
| `SoftDelete` | `Entity::active()` / `including_deleted()` scopes | `WHERE deleted_at IS NULL` in SELECT, `UPDATE SET deleted_at = NOW()` for delete |
| `TenantIsolation` | `platform_organization_id` FK column + RLS session vars | `WHERE tenant_column = :tenant_id` on every query |
| `RowSecurity` | RLS template with `RowSecurityPolicy` | Inline `USING`/`CHECK` expressions (future) |
| `Audit` | `created_at`, `updated_at`, `updated_by`, `deleted_by` columns | Same columns in RETURNING clauses + trigger queries |
| `Retention` | Column + archive strategy | Time-partitioned WHERE clauses (future) |

### Adding a new persistence provider

1. Add a variant to `PersistenceProvider` in `profile/types.rs`
2. Create generators that consume `build_persistence_entity()` and emit
   provider-specific output (e.g. `diesel_entity.rs`, `sqlx_repo.rs`)
3. Register generators in `registry.rs` entity/generator vecs
4. Add capability entries in `profile/capabilities.rs` `base_capabilities()`
5. Add `persistence_provider` entry in `profiles.toml` features
6. Unit tests + snapshot tests for the new output format

## Deployment Topology System

### Overview

`DeploymentTopology` selects the shape of the generated backend: today's
single-crate axum server (`Monolith`, default) or one Cloudflare Worker per
bounded-context domain behind a gateway (`Workers`). Configuration plumbing
only for now — generator output behavior is unchanged.

### DeploymentTopology enum

Defined at `crates/codegraph-generate/src/profile/types.rs` (next to `PersistenceProvider`):

| Variant | Config value | Backend shape |
|---------|-------------|---------------|
| `Monolith` | `"monolith"` (default) | Single-crate axum server (today's behavior) |
| `Workers` | `"workers"` | One Cloudflare Worker per domain + gateway |

### Profile configuration

```toml
[profiles.default.features]
deployment_topology = "monolith"   # "monolith" (default) | "workers"
```

Unlike `persistence_provider` (which silently defaults on unknown values),
unknown `deployment_topology` values are a hard configuration error in
`BuildPlan::from_profile()`. The value is stored on `BuildPlan` and propagated
to generators via `ProjectConfig.deployment.deployment_topology` (available in Tera
templates as `project.deployment_topology`).

### Per-domain worker config (domains.toml)

All keys on `DomainEntry` in `crates/codegraph-config/src/config/domain.rs` are
optional (`#[serde(default)]`), so existing domains.toml files parse unchanged:

| Key | Type | Default | Semantics |
|-----|------|---------|-----------|
| `worker_name` | `Option<String>` | `{app_name}-{domain}` (via `worker_name_or()`) | Cloudflare Worker name for this domain |
| `custom_domain` | `Option<String>` | None (gateway default route `/{domain}/*`) | Custom domain / route pattern |
| `service_bindings` | `Option<Vec<String>>` | `depends_on` (via `service_bindings_or_depends()`) | Other domain workers this worker can call |
| `hyperdrive_binding` | `Option<String>` | `"HYPERDRIVE"` (via `hyperdrive_binding_or()`) | Hyperdrive binding name |
| `cron_triggers` | `Option<Vec<String>>` | None | Cron expressions for scheduled handlers |
| `remote_include_mode` | `Option<String>` | `"sql"` (via `remote_include_mode_or()`) | `"sql"` or `"http"` — how cross-domain `include` queries are satisfied |
| `webhooks` | `Option<bool>` | `false` (via `webhooks_or(default)`) | Enable webhook endpoint/subscription CRUD + dispatch/delivery on this domain's worker |
| `queue_name` | `Option<String>` | `{app_name}-{domain}-webhooks` (via `queue_name_or()`) | Cloudflare Queue name for webhook delivery jobs (producer + consumer) |
| `queue_binding` | `Option<String>` | `"WEBHOOK_QUEUE"` (via `queue_binding_or()`) | Cloudflare Queue binding name used in `env.queue(binding)` + wrangler |
| `queue_max_retries` | `Option<u32>` | `5` (via `queue_max_retries_or()`) | Max delivery attempts before an endpoint is auto-deactivated |
| `queue_max_concurrency` | `Option<u32>` | None (omitted from wrangler) | Cloudflare Queues consumer `max_concurrency` |
| `observability` | `Option<bool>` | `false` (via `observability_or(default)`) | Workers native observability: emits an `[observability]` wrangler block, installs a console panic hook + wasm tracing subscriber, and logs per-request metrics to the console |

Convenience accessors on `DomainEntry`: `worker_name_or(default)`,
`service_bindings_or_depends()`, `hyperdrive_binding_or(default)`,
`remote_include_mode_or(default)`, `webhooks_or(default)`,
`queue_binding_or(default)`, `queue_name_or(default)`,
`queue_max_retries_or(default)`, `observability_or(default)`.

### Per-domain webhooks (workers topology)

When a domain sets `webhooks = true`, the worker scaffold emits
`webhook_api.rs` + `webhook_router.rs` + `webhook_dispatch.rs` into
`workers/{domain}/src/` and wires them into the worker's axum router. These
use the **cornucopia** provider templates
(`templates/webhook/{api_endpoints_cornucopia,api_router_cornucopia,dispatch_worker}.tera`)
— separate from the monolith's SeaORM `webhook/{api_endpoints,api_router,dispatch}.tera`
which stay byte-identical (the monolith templates are untouched). The worker
templates render with the per-domain `WorkerDomain` context.

- **wasm32**: a `#[event(scheduled)]` cron (`*/1 * * * *`, auto-added) drains
  `events_{domain}` from pgmq → INSERTs `platform.webhook_delivery` → enqueues a
  `DeliveryJob` onto the domain's Cloudflare Queue; a `#[event(queue)]` consumer
  performs the HMAC-signed HTTP POST and records the outcome. Transient failures
  return an error so the queue retries (plus `next_retry_at` scan).
- **native**: a tokio interval loop (`WebhookDispatcher`) drains + delivers
  inline via `reqwest` (no Cloudflare Queue available offline).
- **test pump**: `POST /_dispatch` (in `webhook_router.rs`) synchronously drains
  `events_{domain}` and enqueues (wasm, via a `Send`-safe `run_in_worker_loop`
  bridge) or delivers (native) immediately, so tests don't wait for cron.
- **wrangler**: `worker_wrangler.tera` emits `[[queues.producers]]` +
  `[[queues.consumers]]` (`max_retries`/`max_concurrency`) plus the drain cron.

Cloudflare Queues are **not** available in local `wrangler dev` (non-remote)
mode — use the native loop for offline testing, or `wrangler dev --remote` / a
real deployment for the queue path.

### Workers native observability (workers topology)

When a domain sets `observability = true`, the worker scaffold emits native
Cloudflare Workers Observability (decision #111) — no hand-rolled OTLP:

- **wrangler**: `worker_wrangler.tera` (and `gateway_wrangler.tera`, when any
  domain enables it) emit a minimal `[observability]` block (`enabled = true`,
  `head_sampling_rate = 1`). Off by default, so unconfigured domains stay
  byte-identical to pre-observability output.
- **console logging**: the wasm32 slice installs `console_error_panic_hook`
  (crates.io `0.1`, `[target.'cfg(target_arch = "wasm32")'.dependencies]`) and a
  minimal hand-rolled `tracing::Subscriber` (`worker.tera`) that forwards
  `tracing::info!/warn!/error!` to `worker::console_log!/warn!/error!`. A
  third-party `tracing-wasm` dep was deliberately avoided — it pins a stale
  `wasm-bindgen`, while worker-rs 0.8 ships the console macros unconditionally.
  The native path keeps `tracing_subscriber::fmt::init()`.
- **metrics**: `worker_middleware.tera`'s `metrics_middleware::track_metrics`
  emits a structured `http_request method=… path=… status=… duration_ms=…`
  console line per request (wasm32 only) when observability is enabled. The
   monolith's Prometheus `metrics` recorder (`metrics_middleware.tera`) is
   untouched. Analytics-Engine ingestion remains a future TODO.

## DB-Level Authorization (#169 db-authz branch)

### Overview

Authorization for generated apps is enforced **in the database** — RLS
policies in the same SQL round trip that touches the data — replacing the
per-request middleware authz chain (issue #169). Per request the generated
stack now spends: 1 auth round trip + 1 context bundle + the statements
(down from 3 authz-related wire trips + 2 context trips). HTTP semantics
are preserved: out-of-scope API-key operations still return **403** (via a
custom Postgres error), cross-tenant access still filters silently
(404/`[]`).

### Trip accounting (monolith, per request)

| Stage | Before | After |
|-------|--------|-------|
| Auth (API key) | 2 (`verify_api_key` + usage log) | 1 (+ async usage log) |
| Auth (JWT) | 2 (`resolve_user_org` + `get_current_user_role`) | 1 (`resolve_jwt_context`) |
| Scope check | 0 in-process + 1 (`check_api_key_scope_by_id` in permission middleware) | 0 (RLS in the data statement) |
| Session context | 2 (`set_config` batch + `SET LOCAL ROLE`) | 1 (single bundled payload) |

### app_user pool (`APP_DATABASE_URL`)

The generated server builds a serving pool from `APP_DATABASE_URL` — the
NOBYPASSRLS `app_user` role created by migration `0002`. When set, no
per-transaction `SET LOCAL ROLE` is needed; without it the pool falls back
to the owner `DATABASE_URL` (**legacy mode**) and the bundle carries the
role flip (a no-op when already `app_user`). Boot migrations always run on
the owner connection. `AppState.pool_mode` (`DbPoolMode::AppUser`/`Legacy`)
records the mode; both generated `doctor` and `codegraph doctor` warn in
legacy mode. The ops api suite pins the `app_user` password
(`app_user_pass`, migration default) and exports `APP_DATABASE_URL`.
Workers topology needs no code change: point the Hyperdrive/`DATABASE_URL`
binding at `app_user` credentials to get pool mode.

### Request context bundle

`set_rls_session_vars` (generated `command.rs`/`query.rs`) and the tree
handler deliver ALL session context — `app.current_api_key`,
`app.current_api_key_id`, `app.organization_id`, `app.correlation_id`,
`app.user_id`, `app.role` — plus `SET LOCAL ROLE app_user` in ONE
simple-query payload (`execute_unprepared` / cornucopia `batch_execute`).
Values are typed UUIDs inlined with quote escaping (the simple protocol
takes no bind parameters). One round trip per operation.

### Scope enforcement in RLS (`enforce_api_key_scope`)

`public.enforce_api_key_scope(entity_type, entity_id, action)` resolves the
key from `app.current_api_key_id` (**no bcrypt re-verification per row** —
the old `check_api_key_scope` path re-ran `verify_api_key`'s bcrypt inside
policy evaluation). RESTRICTIVE `scope_enforced_*` policies (AND-combined
with org isolation, `TO app_user, api_key`) are emitted for auditable
tenant tables. API-key sessions without a matching scope raise **P0403**
with an `INSUFFICIENT_SCOPE` JSON payload; row-level filtering continues
below the capability check. Both scope vocabularies are accepted: legacy
objects `{"entity_type","entity_id","action"}` and strings
`{domain}.{entity}.{action}`; `write` covers `create`+`update`. `INSERT ..
RETURNING` also applies the SELECT policy — create-capable keys need
`read` to get the response body.

### Role enforcement in RLS (`enforce_role_action`)

Entities with `permissions.scope` configured in `domains.toml` (the same
gate that used to add the router permission layers) emit RESTRICTIVE
`role_enforced_*` policies calling `public.enforce_role_action(action)` —
the owner/manager/member/employee matrix from the retired
`role_allows()`. The org role rides the bundle (`app.role`, resolved once
at auth time); denial raises **P0403** with a `ROLE_FORBIDDEN` payload.
Entities without permission config keep tenancy-only behaviour.

### 403 mapping

Generated domain errors gain `Forbidden(String)` and `from_repo_err()`,
which classifies repo errors by marker: `INSUFFICIENT_SCOPE` /
`ROLE_FORBIDDEN` (the P0403 payloads) and Postgres' canonical
`violates row-level security policy` (org-isolation writes) → HTTP 403;
everything else stays `InternalError`. The tree handler classifies into
`AppError::forbidden`.

### What was removed

`api/scope.tera` (per-route scope guard, both topologies) and the
permission middleware's per-request `check_api_key_scope_by_id` DB lookup.
The permission middleware remains for JWT role checks in-process until
consumers adopt the role policies; its `RequiredPermission` is a
**named-field** struct (`resource`, `action`) — construct accordingly.

### Tests

```bash
# DB-level authorization contract (needs Postgres; pgmq shim makes it
# portable to plain postgres):
cargo test -p codegraph-ops --test authz_contract -- --ignored --nocapture
# Generated app compile gate (needs protoc):
cargo test -p codegraph --test grafeo_e2e_tests -- grafeo_generated_code_compiles
```

### Known limitations / follow-ups

- `public_operations` remains graph-only config; route-level public
  mounting + PUBLIC RLS policies are a follow-up (issue #169 Q3).
- ActiveModel/SeaORM statement payloads cannot carry the context CTE, so
  the context bundle is 1 trip per operation rather than 0; zero-trip CTE
  bundling applies only if repos move off ActiveModel.
- atproto builds keep the cosmos-extensions AuthorizationService; their
  scope enforcement rides the same RLS policies. Deep atproto DB-authz
  (DID-based grants as RLS) is deferred.
- hr-specs' full e2e (~4,062 specs) is the external acceptance gate;
  403 semantics are preserved by design (custom P0403 errors), so specs
  should pass without updates.

## Policy-Driven RLS (`rls_from_policy`, issue #219)

### Overview

The `policy_rls` global generator turns the rexlang actor policy graph
(`ActorNode` / `CapabilityNode` / `GrantEdge`, ingested from `.actor`
imports — see the rexlang authorization section) into ONE Postgres
migration, `020000_policy_rls.sql` (after the per-entity RLS band). Opt-in
via `rls_from_policy = true` under `[features]` in profiles.toml plus
`"policy_rls"` in an `[api]` generators list; the capability requires the
feature (gRPC/ops precedent) and is skipped on plan-less runs. Flag off ⇒
byte-identical output (enforced by a full-pipeline hash snapshot captured
from pre-feature master).

### Mapping table

| Policy element | SQL effect |
|----------------|-----------|
| human actor (`kind: human` or unspecified) | Postgres group role, snake_case via `codegraph-naming`, created `NOLOGIN` (attach with `GRANT <role> TO <user>`) |
| agent actor | existing gateway role `app_user` (never holds DB credentials) |
| `permit` on capability bound to class C | permissive `CREATE POLICY ... FOR ALL TO <roles> USING (p) WITH CHECK (p)` on C's table + `GRANT SELECT, INSERT, UPDATE, DELETE` |
| multiple permit grants with different `when`s | predicates OR-combined; any unconditional grant ⇒ `true` |
| `forbid` | `REVOKE ALL ON C's table FROM <roles>` (auditable; conditional forbids are refused) |
| class `pkg::Name` | last `::` segment, matched against schema titles exactly, then with the configured `Type` suffix |

### `when` → SQL closed subset

`db/expr_sql.rs` parses the stored expression source with `rex-expr` (parse
only, no model) and lowers: own-table field refs (→ quoted column, resolved
through the class's schema properties), string/number/boolean literals,
`date("YYYY-MM-DD")` literals (→ `DATE '...'`, format-validated; the compared
field is guaranteed date-typed by the upstream rex-driver typecheck),
`==`/`=`/`!=`/`<`/`<=`/`>`/`>=`, `&&`/`||`/`!`, parentheses, and `field ==
null` / `field != null` → `IS NULL` / `IS NOT NULL`. EVERYTHING else —
arithmetic, `?.`, `?:`, `let`, lambdas, collection algebra, date calendar
algebra (`plus_days` etc.), `if`, calls, list literals, dotted navigation,
unknown fields — is a HARD generation error naming the capability (rexlang
Cedar-backend philosophy). Postgres only; sqlite is a documented no-op.
Policy-derived RLS is ADDITIVE with the domains.toml-driven RLS: permissive
policies OR-combine (only widening); RESTRICTIVE policies from `rls.tera`
still AND on top.

## Branch & PR Workflow

- **`master` is the single trunk.** All PRs target `master`; feature branches are
  short-lived (merge within ~2 weeks) and deleted after merge
  (`delete_branch_on_merge` is enabled on the repo).
- No long-lived integration branches: sync trunk into your feature branch with
  `git merge master` instead of maintaining a parallel line.
- CI (`lint`, `test`, `test-ops-integration`) must be green before merging;
  lint runs `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings`.
- Pruning merged branches: `git branch --merged master | xargs -r git branch -d`
  locally; remote heads vanish automatically on merge. Stale/discontinued lines
  are tagged `archive/<name>` before deletion (see `archive/*` tags).
- Worktrees in use: `~/git/codegraph-atproto` (atproto line), `~/git/codegraph-ifml`
  (IFML line), `~/git/codegraph-samm` (SAMM spike).

## Code conventions

- No `unwrap()` in production code. Use `thiserror` + `?` propagation.
- Imports grouped: std → external → internal → current crate, separated by blank lines.
- Templates in `crates/codegraph-generate/templates/` use Tera syntax.
- 60+ generators in `crates/codegraph-generate/src/` organized by target (api, db, ddd, ui, cli, etc.).
- IFML-specific generators in `crates/codegraph-generate/src/ifml/`.
- gRPC-specific generators in `crates/codegraph-generate/src/grpc/`.
- Cornucopia-specific generators in `crates/codegraph-generate/src/db/cornucopia_*.rs` and `crates/codegraph-generate/src/ddd/cornucopia_repo.rs`.
- New node/edge types go in `crates/codegraph-core/src/types/` + `crates/codegraph-grafeo/src/schema_ddl.rs`.
- New GraphIngestor/GraphQuerier trait methods need implementations in Grafeo engine AND MockEngine AND CachingQuerier. In codegraph-grafeo both live as directory modules: `src/querier/` and `src/ingestor/` — family files hold inherent `query_*`/`insert_*` methods (one per domain family, `pub(super)`), and the single trait impl in each `mod.rs` is a thin delegation adapter (coherence forbids split trait impls). New trait methods: add the inherent method to the right family file + one adapter delegation.
- New gRPC generators need registration in `registry.rs`, a capability entry in `profile/capabilities.rs`, and an entry in `profiles.toml`.
- New persistence provider generators need a `PersistenceProvider` variant, generator capability entries, and registration in `registry.rs`.
- New DB generators (or modifications to existing ones) must use the `SqlDialect` trait (see `crates/codegraph-generate/src/db/dialect.rs`) for type mapping and feature gating instead of hardcoding PostgreSQL types.
- When adding new template files for a dialect, place them in `templates/db/<dialect>/` and the generator selects the right template path based on `database_target`.
- The `project.database_target` and `project.persistence_provider` variables are available in all Tera templates via `ProjectConfig`.

## Dependency policy (#426)

- **Cadence**: monthly Tier-0 lockfile refresh (`cargo update`, lockfile-only PR — the #418 shape); quarterly review of Tier-3 majors against the #417 table. Majors are deliberate epic work — no drive-by major bumps.
- **Deliberate pins** (bump only with their stated gate green):
  - `sqlglot-rust =0.10.30` — exact pin for the SQLite parse gate (a sqlglot regression must never reach a migration file).
  - `grafeo`/`grafeo-*` exact `=x.y.z` pins — the graph engine moves as reviewed patch bumps (#420 shape); the #389 query-count pin must not move up.
  - `rex-*` @ a714070 (bumped for the DDD Sculptor semantics, rexlang #47, and the IFML module composition/deep-navigation grammar) and `sigil-*` @ 49a6a27 — rev-pinned git deps; move only in coordination with the atproto/rosetta lines.
  - `tree-sitter = "0.25"` and (no direct) `salsa` — **gated on an auto-lsp release > 0.6.2**: `links = "tree-sitter"` forbids version coexistence and auto-lsp re-exports 0.25 / pins salsa 0.22 internally (documented next to the workspace pins, #422/#423).
- **Emitted-contract majors** (sea-orm, utoipa, and friends pinned inside generated `Cargo.toml`s) ship with: review-fixture regeneration, re-blessed byte-identity baselines, an enumerated diff, and a consumer migration note in the PR (#424/#425 shape). Generated apps link `codegraph-workflow` — the workspace crate and the emitted app must stay on ONE sea-orm major.
- **Byte-identity discipline**: any template commit that changes generated output MUST re-bless the canaries (`ux_rules_pre_feature_tree.sha256`, `policy_rls` snapshot, review fixture) in the SAME PR with an enumerated diff. Master has shipped un-blessed template changes before (#433 catch-up) — don't repeat it.
- **cargo-audit**: runs in CI on every PR (the `audit` job). Known accepted warnings (non-fatal, watch list): `RUSTSEC-2025-0141` (bincode unmaintained — transitive via grafeo) and `RUSTSEC-2026-0308` (salsa 0.23 unsound — via rex-driver; fixed upstream in salsa 0.28.5; moves with the rexlang rev pin).
- **dependabot**: scoped to GitHub Actions only (`.github/dependabot.yml`); Cargo automation is consciously declined for the reasons above.

## Include Path System (`?include=`)

Generated list/GET handlers support `?include=` eager loading. Paths are
resolved at generation time by
`crates/codegraph-generate/src/api/include_path.rs`: `allow_include`
entries in `domains.toml` are explicit paths; everything else comes from
auto-discovery (config children with `parent`/`parent_ref`, graph
entity-refs, parent candidates). Heaviest consumer: hr-specs (its
~4,000-test e2e suite is the acceptance gate for include changes).

### Resolution gates (evaluation order)

- Junction arrays (array-of-entity-ref) are skipped — they materialize as
  junction tables with neither a source FK nor a target back-ref, so fetch
  helpers would reference nonexistent columns (the original #82 bug).
- **Config-declared children are authoritative**: when the target is
  `role = "child"` with `parent` + `parent_ref` matching the current
  source, the parent_ref column (e.g. `worker_type_id`) is the reverse FK
  and is exempt from the schema-property gate — the child's schema JSON
  carries no such property (#143).
- Force-VO, codelist, and non-entity targets are skipped.
- A path whose segments were all skipped emits nothing; requesting it at
  runtime is a 400 `Unknown include path` (the allow-list is exactly the
  resolved paths).

### Emitted code (repository_emitter + handler.tera)

- One single fetch (GET-by-id) + one batch fetch (list) per single-segment
  path; `emit_dot_fetch_method` (combined intermediate+leaf response) per
  dot path.
- LIST endpoints wire dot paths through `emit_dot_batch_fetch_method`
  (per-source-id loop over the dot fetch) and merge each leaf field into
  `included.<segments[0]>[<source_id]>` so sibling dot paths accumulate
  (#155). Before #155, dot paths validated but were never fetched.
- Target child tables hydrate when the include response type is
  entity-native (`{Target}Response`); scoped responses (e.g.
  `WorkerPersonLegalResponse` for `worker.person`) keep all-None children
  — tree struct names don't align with scoped prefixes (#161).
- Nested child response structs are imported from the target entity's
  dto_response module (`use crate::domain::{dom}::{tbl}::dto_response::
  {Struct}Response;`, #156–#160). Target trees are built before the import
  block so these names reach `resolve_imports`' siblings.

### Tests

- `include_junction_tests` — junction skip (explicit + auto), parent_ref
  gate, config-child fetchability.
- `template_harness::dot_include_list_handler_wires_batch_and_merge` and
  `template_harness::person_include_hydrates_target_child_tables`.

Regenerating a real consumer (hr-specs: `cargo build -p hr-graph
--release` then `hr-graph run ...`) is the compile gate for template
changes — include-template errors only surface when the generated app
builds.

## Cross-Domain Schema Deduplication

### Problem

When a JSON schema type exists in multiple domains via `allOf` extension
(e.g., `PositionType` in `common/` and `screening/`), the ingestion step
flattens allOf properties onto the extension schema. Both Schema nodes
then share copies of the same properties. The graph querier's
`get_properties(title)` returns properties from ALL Schema nodes with that
title, producing duplicates.

This causes:
- Duplicate FK constraints and COMMENT blocks in DDL migrations (3×+)
- Duplicate migration files (one per domain for the same entity)
- Missing child tables when the allOf chain involves entity-like VO types

### Fixed in commits `3305f8b` → `3e82ec6`

Three fixes work together:

1. **`querier/composition.rs`** — Call `root.dedup_fields()` on the composition tree
   root in `get_composition_tree()`. Removes duplicate columns and children
   at the source.

2. **`db/ddl/query.rs`** — ForeignKey and ColumnComment deduplication in
   `query_ddl_context()`. FKs deduped by `column_name`, comments by `column`.

3. **`ordering.rs`** — `seen_titles` HashSet in `compute_generation_order()`
   tracks entity titles across domains. A title assigned to a higher-priority
   domain is skipped in subsequent domains.

### `dedup_fields()` — Critical regression risk

**File**: `crates/codegraph-core/src/types/composition.rs`

The `dedup_fields()` method MUST use **independent HashSets** per category
(columns, jsonb_columns, children). A shared set would silently remove child
`CompositionNode`s when a column and child share the same `field_name`.

This happens with VO→entity allOf patterns (commit `33240aa`), where
`build_composition_node()` pushes both an FK column and a child node for
the same property. With a shared HashSet, the column (processed first)
blocks the child from being retained.

Fixed in commit `3e82ec6` (independent HashSets per category).

### Composition recursion query budget (issue #389)

`get_composition_tree` (grafeo `querier/composition.rs`) runs per-node graph
reads through `CompositionLoadout`, a per-tree memo so each distinct lookup
(schema, properties, codelist/composite/ref-target/array-item, consumed
fields, allOf edges) executes at most once per tree build — sibling VOs of
the same type and allOf revisits replay cached results. `dedup_fields` and
visited/depth semantics are untouched. The query-count pin test
(`query_count_tests` in composition.rs, AtomicU64 at the `query_gql`/
`query_gql_params` choke point) asserts 47 GQL executions on its
representative fixture (was 58 pre-memo) — if you change the recursion,
re-measure and update the pin; the count must only go down.

## SeaORM JSONB INSERT Workaround

### Problem

`Statement::from_sql_and_values()` with parameterized binding silently drops
JSONB column values on INSERT. Returns `Ok(rows_affected=1)` but the row
either isn't persisted or has NULL/empty JSONB columns. UUID and TEXT
columns are unaffected.

### Workaround

Use `Statement::from_string()` with inline formatted SQL. The pattern:

```rust
let json_val = serde_json::to_string(&value)
    .unwrap_or_default()
    .replace('\'', "''");
let sql = format!(
    "INSERT INTO platform.webhook_endpoint (..., headers) VALUES (..., '{}'::jsonb, ...)",
    json_val
);
db.execute(Statement::from_string(DatabaseBackend::Postgres, sql)).await?;
```

### Affected templates

| Template | INSERTs needing workaround |
|----------|---------------------------|
| `templates/webhook/api_endpoints.tera` | `create_endpoint`, `create_subscription` |
| `templates/webhook/dispatch.tera` | Delivery creation (already has workaround) |

### Known issue

`issue-01.md` in the hr-specs repo documents this as a general sea_orm bug.
The workaround is SQL-injection-prone (string values must manually escape
single quotes). A proper fix would replace sea_orm's `DatabaseConnection`
with a direct `sqlx::PgPool` for dispatch workers.

## Webhook E2E Test Patterns

### Playwright selector best practices

Generated E2E tests use Playwright locators. Several patterns cause flaky
failures due to substring matching:

| Problem | Bad | Good |
|---------|-----|------|
| Text matches URLs and nav | `getByText('Webhooks')` | `getByRole('heading', { name: 'Webhooks' })` |
| Text matches description text | `click('text=Edit')` | `click('text="Edit"')` (exact match) |
| Multi-element strict mode | `getByText('Failed')` | `getByText('Failed').first()` |

`getByText()` and `text=` are **substring** matchers. They match nav links,
endpoint URLs containing the search string, and description text. Use
`getByRole()` for unique elements, exact text quotes for buttons, and
`.first()` for delivery logs that accumulate entries from parallel tests.

### Webhook delivery test flakiness

The "retries failed delivery" test was flaky for two reasons:

1. **Broad subscription** (`event_entity: null, event_type: null`) matched
   all timecard events from ALL parallel tests, accumulating 42+ delivery
   records. Fixed by narrowing to `event_entity: 'timecard', event_type: 'created'`.

2. **Multi-element strict mode** — 42 delivery log badges all matched
   `getByText('Failed')`. Fixed with `.first()`.

### Template files

| Template | Purpose |
|----------|---------|
| `templates/ui/test/webhooks_crud.test.tera` | CRUD E2E tests |
| `templates/ui/test/webhooks_delivery.test.tera` | Delivery E2E tests |
| `templates/ui/test/webhooks_advanced.test.tera` | Advanced E2E tests (not wired into generator) |
| `templates/ui/scaffold/settings_webhooks.tera` | SvelteKit list page |
| `templates/ui/scaffold/settings_webhook_detail.tera` | SvelteKit detail page |
| `templates/ui/scaffold/settings_webhook_form.tera` | SvelteKit create/edit form |

### Svelte page `data-testid` requirement

The webhook form template MUST include `data-testid="webhook-endpoint-submit-btn"`
on the submit button. Tests wait for hydration of this selector before
interacting with the form. If the attribute is missing (from an outdated
template), all form-submit tests fail with `waitForURL` timeouts.

## DDL Code Generation Architecture

### Composition tree flow

```
JSON Schema → Classifier → Graph Nodes → CompositionTree → DDL Context → Templates → SQL
```

Key files:

| File | Role |
|------|------|
| `crates/codegraph-classifier/src/classify.rs` | Classification: ValueObject vs StructuredWrapper vs Entity vs Codelist |
| `crates/codegraph-core/src/types/composition.rs` | `CompositionNode`, `CompositionTree`, `dedup_fields()` |
| `crates/codegraph-grafeo/src/querier/` | `build_composition_node()`, `get_composition_tree()`, `get_properties()` |
| `crates/codegraph-generate/src/db/ddl/query.rs` | `query_ddl_context()`, `column_info_to_ddl()`, `composition_node_to_child_table()`, FK/Comment dedup |
| `crates/codegraph-generate/src/ordering.rs` | `compute_generation_order()`, domain-level entity dedup |
| `crates/codegraph-generate/templates/db/table.tera` | PostgreSQL DDL template with child table rendering |

### How structured fields become child tables

1. Classifier assigns `ValueObject` classification (or explicit `force_value_objects`)
2. `build_composition_node()` creates a `CompositionNode` child for the VO property
3. `composition_node_to_child_table()` converts child → `ChildTableDef`
   with table name `{parent_table}_{field_name}`
4. `flatten_child_tables()` recursively flattens tree, skipping duplicates by name
5. `table.tera` renders `{% for child in child_tables %}` creating separate DDL tables

### AllOf → VO→entity extender pattern

When a ValueObject type extends an entity via allOf (e.g., `RemoteWorkType` allOf
→ `RemoteWork` entity), `build_composition_node()` pushes BOTH an FK column
(`remote_work_id UUID`) AND a child CompositionNode. The entity generator
correctly produces both outputs. The DDL generator must also produce both
(FK column + child table). `dedup_fields()` with independent HashSets
preserves both.

### Child-table projection contract (issue #460)

One derivation rule across every plane: **a composition edge lowers to an FK
on the child side.** A contained value object (contains-only, no referring
edges) projects as a `{parent}_{feature}` child table; a contains-target that
is also a refers target ("refers wins" → entity) projects as the synthetic
`{parent}_id` back-ref FK **on the entity's own table** (DDL via the
`ArrayItems` ParentCandidate through `add_parent_fk`, CASCADE; the repo plane
mirrors it with `ChildTableInfo.is_back_ref` — create-and-link, back-ref
hydration, `ORDER BY id`). Naming flows through two `codegraph-naming`
helpers — `child_table_name(parent, feature)` and
`child_parent_fk_column(parent)` (suffix-aware, moved from codegraph-core) —
used by DDL, repository, entity, junction, codelist, and filter planes;
duplicate `format!` derivations are gone.

Enforcement: `crates/codegraph-generate/src/consistency.rs`
(`check_child_table_consistency`, run from `pipeline.rs` when the profile
carries the `ddl` generator) builds both planes' table universes and
hard-errors (`Config`) when repository SQL targets a table the DDL never
creates — the pre-fix failure was `person_legal_documents` INSERTs against a
`legal_document`-only DDL (500 on every person create/list). Ops-less trees
(e.g. a `.ddd`-designed VO whose design carries no repository) emit no SQL
and are skipped. mox-first FK retention: `generated_table_set` now unions
the graph's entity tables with the config `entities` lists, so previously
dropped FK constraints (all of them, mox-first having no entities key)
survive — flag-off canaries re-blessed for exactly this (2 migration files
gain guarded FK-constraint blocks). Gate: `cargo test -p codegraph --test
mox_child_table_consistency_tests`.
