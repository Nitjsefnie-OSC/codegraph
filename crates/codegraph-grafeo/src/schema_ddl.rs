/// Version of the graph schema/document format produced by this binary.
/// Persisted graphs and exported artifacts carry it; a reader refusing a
/// NEWER version than its own constant keeps old binaries honest, and a
/// writer bumping it invalidates persisted-graph caches deterministically.
///
/// NOT bumped for issue #279 (`Schema.access`/`Schema.annotations`): the
/// properties are nullable, old persisted graphs read them as NULL
/// (→ `None`), and inserts of the new properties are tolerated on
/// pre-existing node types — verified against the grafeo engine.
pub const GRAPH_FORMAT_VERSION: u32 = 1;

/// Returns GQL DDL statements for node types and edge types.
/// These match the `codegraph-core` shared types (derived from `db/schema.hx`).
pub fn ddl_statements() -> Vec<&'static str> {
    let mut stmts = Vec::new();
    stmts.extend(node_type_ddl());
    stmts.extend(edge_type_ddl());
    stmts
}

/// Returns property names that should be indexed via `GrafeoDB::create_property_index()`.
pub fn indexed_properties() -> Vec<&'static str> {
    vec![
        "title",
        "schema_id",
        "_schema_title",
        "_schema_id",
        "name",
        "_codelist_name",
        "nsid",
        "authority",
        "fqn",
        "did",
        "target_schema",
        "source_schema",
        "subject",
        "identity",
        "tenant",
    ]
}

fn node_type_ddl() -> Vec<&'static str> {
    vec![
        // AtprotoNamespace — AT Protocol repo namespaces. Renamed from
        // `Namespace` (issue #267 collision resolution) so the graph-wide
        // namespace concept owns the plain label.
        "CREATE NODE TYPE IF NOT EXISTS AtprotoNamespace (
            authority STRING NOT NULL,
            segment STRING NOT NULL,
            domain STRING NOT NULL
        )",
        // Namespace — graph-wide namespaces (issue #267). `fqn` is the
        // unique key; `parent` is persisted flat (and as a NamespaceParent
        // edge); `source` records the declaration provenance ("config",
        // "discovered", "mox", ...).
        "CREATE NODE TYPE IF NOT EXISTS Namespace (
            fqn STRING NOT NULL,
            parent STRING,
            source STRING
        )",
        // Lexicon — AT Protocol
        "CREATE NODE TYPE IF NOT EXISTS Lexicon (
            nsid STRING NOT NULL,
            lex_type STRING NOT NULL,
            key_strategy STRING NOT NULL,
            revision INTEGER,
            description STRING,
            domain STRING NOT NULL
        )",
        // Collection — AT Protocol
        "CREATE NODE TYPE IF NOT EXISTS Collection (
            nsid STRING NOT NULL,
            key_strategy STRING NOT NULL,
            domain STRING NOT NULL
        )",
        // Repository — AT Protocol
        "CREATE NODE TYPE IF NOT EXISTS Repository (
            did STRING NOT NULL,
            handle STRING,
            pds_endpoint STRING NOT NULL,
            org_name STRING NOT NULL,
            tenancy_mode STRING NOT NULL
        )",
        // SchemaNode — 25 fields from codegraph-core/src/types/schema.rs
        "CREATE NODE TYPE IF NOT EXISTS Schema (
            schema_id STRING NOT NULL,
            title STRING NOT NULL,
            description STRING,
            schema_type STRING NOT NULL,
            classification STRING NOT NULL,
            pg_type STRING NOT NULL,
            rust_type STRING NOT NULL,
            sea_orm_type STRING NOT NULL,
            domain STRING,
            namespace STRING,
            rel_path STRING NOT NULL,
            rust_type_name STRING NOT NULL,
            pg_table_name STRING NOT NULL,
            api_path_segment STRING NOT NULL,
            parent_schema STRING,
            is_entity BOOLEAN NOT NULL,
            is_codelist BOOLEAN NOT NULL,
            is_primitive_wrapper BOOLEAN NOT NULL,
            has_all_of BOOLEAN NOT NULL,
            has_one_of BOOLEAN NOT NULL,
            has_any_of BOOLEAN NOT NULL,
            has_definitions BOOLEAN NOT NULL,
            custom_annotations STRING NOT NULL,
            access STRING,
            annotations STRING
        )",
        // PropertyNode — 16 fields + scalar/item bounds + _schema_title
        // denormalized. Bounds are persisted as STRING (u64/Decimal serialize
        // via to_string) because Decimal has no native grafeo Value;
        // conversions parses back.
        "CREATE NODE TYPE IF NOT EXISTS Property (
            name STRING NOT NULL,
            prop_type STRING NOT NULL,
            description STRING,
            format STRING,
            is_required BOOLEAN NOT NULL,
            is_nullable BOOLEAN NOT NULL,
            is_array BOOLEAN NOT NULL,
            pattern STRING,
            min_length STRING,
            max_length STRING,
            min_items STRING,
            max_items STRING,
            minimum STRING,
            maximum STRING,
            pg_column_name STRING NOT NULL,
            pg_column_type STRING NOT NULL,
            rust_field_name STRING NOT NULL,
            rust_field_type STRING NOT NULL,
            sea_orm_type STRING NOT NULL,
            render_strategy STRING NOT NULL,
            ref_target STRING,
            classification STRING,
            _schema_title STRING NOT NULL,
            _schema_id STRING
        )",
        // CodeList — 5 fields
        "CREATE NODE TYPE IF NOT EXISTS CodeList (
            name STRING NOT NULL,
            description STRING,
            pg_table_name STRING NOT NULL,
            render_as STRING NOT NULL,
            check_expression STRING
        )",
        // EnumValue — 3 fields + _codelist_name denormalized
        "CREATE NODE TYPE IF NOT EXISTS EnumValue (
            value STRING NOT NULL,
            display_name STRING,
            sort_order INTEGER NOT NULL,
            _codelist_name STRING NOT NULL
        )",
        // CompositeColumn — 5 fields
        "CREATE NODE TYPE IF NOT EXISTS CompositeColumn (
            suffix STRING NOT NULL,
            pg_type STRING NOT NULL,
            rust_type STRING NOT NULL,
            sea_orm_type STRING NOT NULL,
            fk_target STRING
        )",
        // CompositeRange — 6 fields
        "CREATE NODE TYPE IF NOT EXISTS CompositeRange (
            pg_column_name STRING NOT NULL,
            pg_type STRING NOT NULL,
            rust_type STRING NOT NULL,
            start_field STRING NOT NULL,
            end_field STRING NOT NULL,
            open_end BOOLEAN NOT NULL
        )",
        // Extension — 1 field
        "CREATE NODE TYPE IF NOT EXISTS Extension (
            name STRING NOT NULL
        )",
        // Domain — 1 field
        "CREATE NODE TYPE IF NOT EXISTS Domain (
            name STRING NOT NULL
        )",
        // ViewContainer — IFML
        "CREATE NODE TYPE IF NOT EXISTS ViewContainer (
            name STRING NOT NULL,
            label STRING,
            is_xor BOOLEAN NOT NULL DEFAULT false,
            is_default BOOLEAN NOT NULL DEFAULT false,
            is_landmark BOOLEAN NOT NULL DEFAULT false,
            is_modal BOOLEAN NOT NULL DEFAULT false,
            domain STRING
        )",
        // ViewComponent — IFML
        "CREATE NODE TYPE IF NOT EXISTS ViewComponent (
            name STRING NOT NULL,
            component_type STRING NOT NULL,
            mode STRING,
            entity STRING,
            fields STRING,
            filter STRING,
            api_operation STRING,
            spec STRING,
            domain STRING
        )",
        // Event — IFML
        "CREATE NODE TYPE IF NOT EXISTS Event (
            name STRING NOT NULL,
            event_type STRING NOT NULL,
            params STRING,
            domain STRING
        )",
        // Action — IFML
        "CREATE NODE TYPE IF NOT EXISTS ActionNode (
            name STRING NOT NULL,
            domain STRING
        )",
        // ParameterDefinition — IFML
        "CREATE NODE TYPE IF NOT EXISTS ParameterDefinition (
            name STRING NOT NULL,
            direction STRING NOT NULL,
            type_ref STRING NOT NULL,
            domain STRING
        )",
        // DataBinding — IFML
        "CREATE NODE TYPE IF NOT EXISTS DataBinding (
            name STRING NOT NULL,
            conditional_expression STRING,
            expression_language STRING NOT NULL DEFAULT 'ifml',
            domain STRING
        )",
        // ModuleDefinition — IFML
        "CREATE NODE TYPE IF NOT EXISTS ModuleDefinition (
            name STRING NOT NULL,
            domain STRING
        )",
        // ApiResource — API metamodel
        "CREATE NODE TYPE IF NOT EXISTS ApiResource (
            name STRING NOT NULL,
            schema_title STRING NOT NULL,
            domain STRING NOT NULL,
            label STRING,
            path_segment STRING NOT NULL
        )",
        // ApiOperation — API metamodel
        "CREATE NODE TYPE IF NOT EXISTS ApiOperation (
            name STRING NOT NULL,
            kind STRING NOT NULL,
            input_schema STRING,
            output_schema STRING NOT NULL,
            paging BOOLEAN NOT NULL DEFAULT false,
            sorting BOOLEAN NOT NULL DEFAULT false,
            filtering BOOLEAN NOT NULL DEFAULT false,
            domain STRING
        )",
        // Interaction — API metamodel
        "CREATE NODE TYPE IF NOT EXISTS Interaction (
            name STRING NOT NULL,
            transport STRING NOT NULL,
            domain STRING
        )",
        // HttpEndpoint — API metamodel
        "CREATE NODE TYPE IF NOT EXISTS HttpEndpoint (
            name STRING NOT NULL,
            method STRING NOT NULL,
            path_template STRING NOT NULL,
            domain STRING
        )",
        // Pipeline — API metamodel
        "CREATE NODE TYPE IF NOT EXISTS Pipeline (
            name STRING NOT NULL,
            middleware STRING,
            domain STRING
        )",
        // ErrorDefinition — API metamodel
        "CREATE NODE TYPE IF NOT EXISTS ErrorDefinition (
            code STRING NOT NULL,
            description STRING NOT NULL,
            http_status INTEGER NOT NULL,
            domain STRING
        )",
        // Permission — API metamodel
        "CREATE NODE TYPE IF NOT EXISTS Permission (
            name STRING NOT NULL,
            domain STRING
        )",
        // Policy — persistence metamodel
        "CREATE NODE TYPE IF NOT EXISTS Policy (
            name STRING NOT NULL,
            kind_json STRING NOT NULL,
            target_schema STRING NOT NULL,
            domain STRING
        )",
        // Relationship — persistence metamodel
        "CREATE NODE TYPE IF NOT EXISTS Relationship (
            name STRING NOT NULL,
            source_schema STRING NOT NULL,
            target_schema STRING NOT NULL,
            cardinality STRING NOT NULL,
            ownership STRING NOT NULL,
            fk_json STRING NOT NULL,
            propagation_json STRING NOT NULL,
            domain STRING
        )",
        // SecurityIdentity — persistence metamodel
        "CREATE NODE TYPE IF NOT EXISTS SecurityIdentity (
            name STRING NOT NULL,
            subject STRING NOT NULL,
            domain STRING
        )",
        // Membership — persistence metamodel
        "CREATE NODE TYPE IF NOT EXISTS Membership (
            identity STRING NOT NULL,
            tenant STRING NOT NULL,
            status STRING NOT NULL,
            roles_json STRING NOT NULL,
            valid_from STRING NOT NULL,
            valid_until STRING NOT NULL
        )",
        // Tenant — persistence metamodel
        "CREATE NODE TYPE IF NOT EXISTS Tenant (
            name STRING NOT NULL,
            label STRING NOT NULL,
            strategy_json STRING NOT NULL,
            domain STRING
        )",
        // Actor — authorization metamodel
        "CREATE NODE TYPE IF NOT EXISTS Actor (
            name STRING NOT NULL,
            kind STRING,
            extends STRING,
            block STRING
        )",
        // Capability — authorization metamodel
        "CREATE NODE TYPE IF NOT EXISTS Capability (
            name STRING NOT NULL,
            class STRING NOT NULL,
            block STRING
        )",
        // ActorPolicy — authorization metamodel model-level carrier
        "CREATE NODE TYPE IF NOT EXISTS ActorPolicy (
            name STRING NOT NULL,
            blocks STRING,
            never_both STRING
        )",
        // Vocabulary — mox domain metamodel
        "CREATE NODE TYPE IF NOT EXISTS Vocabulary (
            name STRING NOT NULL,
            package STRING NOT NULL,
            source STRING NOT NULL,
            version STRING,
            key_facet STRING NOT NULL,
            facets_json STRING NOT NULL,
            entries_json STRING NOT NULL
        )",
        // MoxPackage — mox domain metamodel package carrier
        "CREATE NODE TYPE IF NOT EXISTS MoxPackage (
            name STRING NOT NULL
        )",
        // Operation — mox domain metamodel (class behavior, verbatim bodies)
        "CREATE NODE TYPE IF NOT EXISTS Operation (
            name STRING NOT NULL,
            class STRING NOT NULL,
            package STRING NOT NULL,
            description STRING,
            return_type STRING NOT NULL,
            params_json STRING NOT NULL,
            bodies_json STRING NOT NULL
        )",
        // DerivedFeature — mox domain metamodel (computed, never stored)
        "CREATE NODE TYPE IF NOT EXISTS DerivedFeature (
            name STRING NOT NULL,
            class STRING NOT NULL,
            package STRING NOT NULL,
            type_ref STRING NOT NULL,
            expr STRING
        )",
        // Condition — constraint plane (issue #261). `options` persists the
        // one_of option titles as a JSON array string; `expr_json` carries
        // the canonical Expr::to_json() payload for named conditions.
        "CREATE NODE TYPE IF NOT EXISTS Condition (
            name STRING NOT NULL,
            owner_title STRING NOT NULL,
            kind STRING NOT NULL,
            expr_json STRING,
            options STRING,
            definition STRING,
            domain STRING
        )",
        // Regulatory — regulatory reference plane (issue #265). ONE
        // parameterized node type (the Condition precedent) carrying the
        // seven regulatory kinds (report/body/corpus/segment/rule source/
        // rule schema/meta type) in `kind`; the disjoint per-kind payloads
        // persist as one JSON object string in `properties_json`.
        "CREATE NODE TYPE IF NOT EXISTS Regulatory (
            name STRING NOT NULL,
            kind STRING NOT NULL,
            label STRING,
            definition STRING,
            domain STRING,
            properties_json STRING
        )",
        // Function — computation plane (issue #263). ONE structured node
        // per rosetta func: the common shape persists flat (name, domain,
        // definition, extends_function); the typed structured payload
        // (dispatch head, inputs, output, aliases, operations,
        // post-conditions, transform annotations, open-ended metadata)
        // persists as one JSON object string in `payload_json` — Grafeo
        // node properties are a flat bag, so nested Vecs serialize (the
        // ConditionNode `options`/RegulatoryNode `properties_json`
        // precedent).
        "CREATE NODE TYPE IF NOT EXISTS Function (
            name STRING NOT NULL,
            domain STRING,
            definition STRING,
            extends_function STRING,
            payload_json STRING NOT NULL
        )",
        // Rule — computation plane (issue #264). ONE structured node per
        // rosetta reporting/eligibility rule: the common shape persists
        // flat (name, domain, definition, kind, input_type); the payload
        // (expr_json, open-ended metadata) persists as one JSON object
        // string in `payload_json` (the FunctionNode precedent).
        "CREATE NODE TYPE IF NOT EXISTS Rule (
            name STRING NOT NULL,
            domain STRING,
            definition STRING,
            kind STRING NOT NULL,
            input_type STRING,
            payload_json STRING NOT NULL
        )",
        // DDD design plane (issue #449). Seven node families mirroring the
        // rex-ir `.ddd` artifact; service/search sub-structures persist as
        // JSON-string properties (the ActorPolicy singleton pattern), while
        // repository operations are their own node family (repositories are
        // cross-queried).
        "CREATE NODE TYPE IF NOT EXISTS DddApplication (
            name STRING NOT NULL,
            base STRING,
            source_path STRING NOT NULL
        )",
        "CREATE NODE TYPE IF NOT EXISTS DddModule (
            application STRING NOT NULL,
            name STRING NOT NULL,
            ordinal INTEGER NOT NULL
        )",
        "CREATE NODE TYPE IF NOT EXISTS DddDesign (
            application STRING NOT NULL,
            module STRING NOT NULL,
            class STRING NOT NULL,
            resolved_title STRING,
            stereotype STRING NOT NULL,
            is_abstract BOOLEAN NOT NULL,
            flags_json STRING NOT NULL,
            ordinal INTEGER NOT NULL
        )",
        "CREATE NODE TYPE IF NOT EXISTS DddRepository (
            application STRING NOT NULL,
            name STRING NOT NULL,
            design_class STRING NOT NULL
        )",
        "CREATE NODE TYPE IF NOT EXISTS DddRepositoryOperation (
            application STRING NOT NULL,
            repository_name STRING NOT NULL,
            name STRING NOT NULL,
            builtin STRING,
            return_type_json STRING,
            return_multiplicity STRING,
            params_json STRING NOT NULL,
            ordinal INTEGER NOT NULL
        )",
        "CREATE NODE TYPE IF NOT EXISTS DddService (
            application STRING NOT NULL,
            module STRING NOT NULL,
            name STRING NOT NULL,
            description STRING,
            dependencies_json STRING NOT NULL,
            operations_json STRING NOT NULL,
            ordinal INTEGER NOT NULL
        )",
        "CREATE NODE TYPE IF NOT EXISTS DddSearch (
            application STRING NOT NULL,
            module STRING NOT NULL,
            name STRING NOT NULL,
            description STRING,
            entity_class STRING NOT NULL,
            entity_title STRING,
            text_json STRING NOT NULL,
            filters_json STRING NOT NULL,
            sorts_json STRING NOT NULL,
            document_json STRING NOT NULL,
            ranking STRING,
            analyzer STRING,
            pagination_json STRING,
            capabilities_json STRING NOT NULL,
            ordinal INTEGER NOT NULL
        )",
        // Event-contract plane (issue #454). Three standalone node families
        // mirroring the rex-ir `.evt` artifact; the intra-file event
        // references (channel publishes, subscription events) and payload
        // fields persist as JSON-string properties (the DddService
        // `operations_json` pattern) — no edge types this time. Unrelated
        // to the IFML `:Event` label.
        "CREATE NODE TYPE IF NOT EXISTS EvtEvent (
            source_path STRING NOT NULL,
            name STRING NOT NULL,
            version STRING,
            fields_json STRING NOT NULL,
            ordinal INTEGER NOT NULL
        )",
        "CREATE NODE TYPE IF NOT EXISTS EvtChannel (
            source_path STRING NOT NULL,
            name STRING NOT NULL,
            publishes_json STRING NOT NULL,
            ordinal INTEGER NOT NULL
        )",
        "CREATE NODE TYPE IF NOT EXISTS EvtSubscription (
            source_path STRING NOT NULL,
            name STRING NOT NULL,
            events_json STRING NOT NULL,
            consumer STRING NOT NULL,
            ordinal INTEGER NOT NULL
        )",
        // Deployment plane (rexlang #49). ONE node per `.deploy` model (the
        // model is small and consumers always want it whole — the
        // FunctionNode payload_json pattern): `source_path` is the upsert
        // key and `model_json` carries the whole `DeployModelGraph`
        // serialized (every sub-type is Serialize/Deserialize), so no node
        // families or edge types this time.
        "CREATE NODE TYPE IF NOT EXISTS DeployModel (
            source_path STRING NOT NULL,
            model_json STRING NOT NULL
        )",
    ]
}

fn edge_type_ddl() -> Vec<&'static str> {
    vec![
        "CREATE EDGE TYPE IF NOT EXISTS HasProperty (sort_order INTEGER)",
        "CREATE EDGE TYPE IF NOT EXISTS ReferencesSchema (ref_path STRING, resolved_classification STRING)",
        "CREATE EDGE TYPE IF NOT EXISTS ExtendsSchema (composition_type STRING)",
        "CREATE EDGE TYPE IF NOT EXISTS DependsOn (dependency_type STRING)",
        "CREATE EDGE TYPE IF NOT EXISTS HasEnumValue",
        "CREATE EDGE TYPE IF NOT EXISTS UsesCodeList (render_as STRING)",
        "CREATE EDGE TYPE IF NOT EXISTS ExpandsTo (sort_order INTEGER)",
        "CREATE EDGE TYPE IF NOT EXISTS CollapsesTo",
        "CREATE EDGE TYPE IF NOT EXISTS ConsumesField (role STRING)",
        "CREATE EDGE TYPE IF NOT EXISTS ItemsOf",
        "CREATE EDGE TYPE IF NOT EXISTS RequiresExtension",
        "CREATE EDGE TYPE IF NOT EXISTS InDomain",
        "CREATE EDGE TYPE IF NOT EXISTS DomainDepends (dependency_type STRING)",
        // AT Protocol edge types
        "CREATE EDGE TYPE IF NOT EXISTS InNamespace ()",
        "CREATE EDGE TYPE IF NOT EXISTS ProjectsToLexicon ()",
        "CREATE EDGE TYPE IF NOT EXISTS DefinesCollection ()",
        "CREATE EDGE TYPE IF NOT EXISTS LexiconReferences (ref_path STRING)",
        "CREATE EDGE TYPE IF NOT EXISTS StoredInRepository ()",
        // IFML edge types
        "CREATE EDGE TYPE IF NOT EXISTS ContainsViewContainer (sort_order INTEGER)",
        "CREATE EDGE TYPE IF NOT EXISTS ContainsViewComponent (sort_order INTEGER)",
        "CREATE EDGE TYPE IF NOT EXISTS HasEvent",
        "CREATE EDGE TYPE IF NOT EXISTS NavigationFlow (target_param_binding STRING)",
        "CREATE EDGE TYPE IF NOT EXISTS DataFlow (source_param STRING, target_param STRING)",
        "CREATE EDGE TYPE IF NOT EXISTS HasParameter (direction STRING)",
        "CREATE EDGE TYPE IF NOT EXISTS ParameterBindingGroup",
        "CREATE EDGE TYPE IF NOT EXISTS ParameterBinding",
        "CREATE EDGE TYPE IF NOT EXISTS HasDataBinding",
        "CREATE EDGE TYPE IF NOT EXISTS BindsToEntity",
        "CREATE EDGE TYPE IF NOT EXISTS BindsToProperty (role STRING)",
        "CREATE EDGE TYPE IF NOT EXISTS BindsToOperation",
        "CREATE EDGE TYPE IF NOT EXISTS TriggersAction",
        "CREATE EDGE TYPE IF NOT EXISTS ActionEvent (outcome STRING)",
        "CREATE EDGE TYPE IF NOT EXISTS HasModuleDefinition",
        "CREATE EDGE TYPE IF NOT EXISTS HasViewComponentPart (role STRING)",
        "CREATE EDGE TYPE IF NOT EXISTS HasConditionalExpr",
        // API metamodel edge types
        "CREATE EDGE TYPE IF NOT EXISTS ExposesResource",
        "CREATE EDGE TYPE IF NOT EXISTS BindsToSchema",
        "CREATE EDGE TYPE IF NOT EXISTS HasOperation",
        "CREATE EDGE TYPE IF NOT EXISTS InputBoundTo",
        "CREATE EDGE TYPE IF NOT EXISTS OutputBoundTo",
        "CREATE EDGE TYPE IF NOT EXISTS CanReturnError",
        "CREATE EDGE TYPE IF NOT EXISTS RequiresPermission",
        "CREATE EDGE TYPE IF NOT EXISTS HasInteraction",
        "CREATE EDGE TYPE IF NOT EXISTS BindsHttpEndpoint",
        "CREATE EDGE TYPE IF NOT EXISTS UsesPipeline",
        // Persistence metamodel edge types
        "CREATE EDGE TYPE IF NOT EXISTS HasPolicy",
        "CREATE EDGE TYPE IF NOT EXISTS PolicyAppliesTo",
        "CREATE EDGE TYPE IF NOT EXISTS HasRelationship",
        "CREATE EDGE TYPE IF NOT EXISTS RelationshipSource",
        "CREATE EDGE TYPE IF NOT EXISTS RelationshipTarget",
        "CREATE EDGE TYPE IF NOT EXISTS PolicyOnRelationship",
        "CREATE EDGE TYPE IF NOT EXISTS TenantOwns",
        "CREATE EDGE TYPE IF NOT EXISTS HasMembership",
        "CREATE EDGE TYPE IF NOT EXISTS MembershipInTenant",
        "CREATE EDGE TYPE IF NOT EXISTS HasRole",
        // Authorization metamodel edge types
        "CREATE EDGE TYPE IF NOT EXISTS Grant (effect STRING NOT NULL, when_expr STRING, obligations STRING)",
        // mox domain metamodel edge types
        "CREATE EDGE TYPE IF NOT EXISTS BelongsToClass ()",
        "CREATE EDGE TYPE IF NOT EXISTS VocabularyInPackage ()",
        // Constraint plane edge types (issue #261)
        "CREATE EDGE TYPE IF NOT EXISTS HasCondition ()",
        // Regulatory reference plane edge types (issue #265)
        "CREATE EDGE TYPE IF NOT EXISTS RegulatoryReference (ref_path STRING)",
        "CREATE EDGE TYPE IF NOT EXISTS HasRuleSource ()",
        "CREATE EDGE TYPE IF NOT EXISTS CorpusInBody ()",
        "CREATE EDGE TYPE IF NOT EXISTS DerivesFrom ()",
        // Computation plane edge types (issue #263)
        "CREATE EDGE TYPE IF NOT EXISTS FunctionExtends ()",
        // Rule plane edge types (issue #264)
        "CREATE EDGE TYPE IF NOT EXISTS RuleAppliesTo ()",
        "CREATE EDGE TYPE IF NOT EXISTS RuleReference (ref_path STRING, rule_source STRING)",
        // Namespace plane edge types (issue #267). InNamespace (shared with
        // AT-Protocol) is declared above; NamespaceDepends is derived and
        // never ingested today.
        "CREATE EDGE TYPE IF NOT EXISTS NamespaceParent ()",
        "CREATE EDGE TYPE IF NOT EXISTS NamespaceImports (wildcard BOOLEAN, alias STRING)",
        "CREATE EDGE TYPE IF NOT EXISTS NamespaceDepends ()",
        // DDD design plane edge types (issue #449). DddBindsClass links a
        // DddDesign/DddSearch to its resolved Schema (advisory).
        "CREATE EDGE TYPE IF NOT EXISTS DddHasModule ()",
        "CREATE EDGE TYPE IF NOT EXISTS DddHasDesign ()",
        "CREATE EDGE TYPE IF NOT EXISTS DddHasRepository ()",
        "CREATE EDGE TYPE IF NOT EXISTS DddHasService ()",
        "CREATE EDGE TYPE IF NOT EXISTS DddHasSearch ()",
        "CREATE EDGE TYPE IF NOT EXISTS DddHasOperation ()",
        "CREATE EDGE TYPE IF NOT EXISTS DddBindsClass ()",
    ]
}
