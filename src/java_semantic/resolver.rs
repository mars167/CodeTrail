use std::collections::{BTreeMap, BTreeSet};

use crate::java_semantic::{
    model::{
        DispatchKind, ExtractedJavaFile, JavaCallEdge, JavaFileContribution, JavaOccurrence,
        JavaSemanticData, JavaSemanticManifest, JavaSymbol, JavaSymbolKind, OccurrenceRole,
        RawJavaCall, ResolveConfidence, ResolveStatus,
    },
    parse::{erase_type, last_identifier},
};

#[derive(Clone, Debug)]
pub struct ResolverInput {
    pub manifest: JavaSemanticManifest,
    pub files: Vec<ExtractedJavaFile>,
    pub external_symbols: Vec<JavaSymbol>,
}

#[derive(Clone, Debug)]
struct ResolverTables {
    symbols: Vec<JavaSymbol>,
    by_id: BTreeMap<String, JavaSymbol>,
    type_by_qualified: BTreeMap<String, String>,
    type_by_simple: BTreeMap<String, Vec<String>>,
    methods_by_owner_name_arity: BTreeMap<(String, String, usize), Vec<String>>,
    methods_by_name_arity: BTreeMap<(String, usize), Vec<String>>,
    subtypes_by_super_name: BTreeMap<String, Vec<String>>,
    type_edges: Vec<crate::java_semantic::model::JavaTypeEdge>,
}

pub fn resolve(input: ResolverInput) -> JavaSemanticData {
    let mut symbols = input.external_symbols;
    let mut occurrences = Vec::new();
    let mut raw_calls = Vec::new();
    let mut type_edges = Vec::new();
    let mut file_contributions = Vec::new();

    for file in input.files {
        let symbol_count = file.symbols.len();
        let call_edge_count = file.raw_calls.len();
        let occurrence_start = occurrences.len();
        raw_calls.extend(file.raw_calls);
        type_edges.extend(file.type_edges);
        for symbol in &file.symbols {
            if let (Some(path), Some(range)) = (&symbol.path, &symbol.range) {
                occurrences.push(JavaOccurrence {
                    path: path.clone(),
                    range: range.clone(),
                    role: OccurrenceRole::Definition,
                    symbol_id: symbol.symbol_id.clone(),
                    enclosing_symbol: symbol.owner_symbol.clone(),
                    syntax_kind: format!("{:?}", symbol.kind),
                    source: match symbol.origin {
                        crate::java_semantic::model::SymbolOrigin::GeneratedSource => {
                            "generated_source".to_string()
                        }
                        crate::java_semantic::model::SymbolOrigin::LombokSynthetic => {
                            "lombok_synthetic".to_string()
                        }
                        _ => "source".to_string(),
                    },
                    confidence: symbol.confidence,
                });
            }
        }
        file_contributions.push(JavaFileContribution {
            path: file.path,
            file_hash: file.file_hash,
            symbol_count,
            occurrence_count: occurrences.len() - occurrence_start,
            call_edge_count,
        });
        symbols.extend(file.symbols);
    }

    let tables = ResolverTables::new(symbols, type_edges);
    let mut call_edges = Vec::new();
    for raw in raw_calls {
        call_edges.push(resolve_call(&tables, raw));
    }
    let mut manifest = input.manifest;
    manifest.symbol_count = tables.symbols.len();
    manifest.occurrence_count = occurrences.len();
    manifest.call_edge_count = call_edges.len();
    manifest.type_edge_count = tables.type_edges.len();
    manifest.file_count = file_contributions.len();

    JavaSemanticData {
        manifest,
        symbols: tables.symbols,
        occurrences,
        call_edges,
        type_edges: tables.type_edges,
        file_contributions,
    }
}

impl ResolverTables {
    fn new(
        mut symbols: Vec<JavaSymbol>,
        type_edges: Vec<crate::java_semantic::model::JavaTypeEdge>,
    ) -> Self {
        symbols.sort_by(|a, b| a.symbol_id.cmp(&b.symbol_id));
        symbols.dedup_by(|a, b| a.symbol_id == b.symbol_id);
        let mut by_id = BTreeMap::new();
        let mut type_by_qualified = BTreeMap::new();
        let mut type_by_simple = BTreeMap::<String, Vec<String>>::new();
        let mut methods_by_owner_name_arity =
            BTreeMap::<(String, String, usize), Vec<String>>::new();
        let mut methods_by_name_arity = BTreeMap::<(String, usize), Vec<String>>::new();
        let mut subtypes_by_super_name = BTreeMap::<String, Vec<String>>::new();

        for symbol in &symbols {
            by_id.insert(symbol.symbol_id.clone(), symbol.clone());
            if matches!(
                symbol.kind,
                JavaSymbolKind::Type | JavaSymbolKind::Annotation
            ) {
                type_by_qualified.insert(symbol.qualified_name.clone(), symbol.symbol_id.clone());
                type_by_simple
                    .entry(symbol.name.clone())
                    .or_default()
                    .push(symbol.symbol_id.clone());
            }
            if matches!(
                symbol.kind,
                JavaSymbolKind::Method
                    | JavaSymbolKind::Constructor
                    | JavaSymbolKind::SyntheticMethod
            ) {
                if let Some(owner) = &symbol.owner_symbol {
                    methods_by_owner_name_arity
                        .entry((
                            owner.clone(),
                            method_lookup_name(symbol),
                            symbol.parameters.len(),
                        ))
                        .or_default()
                        .push(symbol.symbol_id.clone());
                }
                methods_by_name_arity
                    .entry((method_lookup_name(symbol), symbol.parameters.len()))
                    .or_default()
                    .push(symbol.symbol_id.clone());
            }
        }
        for edge in &type_edges {
            let simple = last_identifier(&edge.supertype);
            for key in [edge.supertype.clone(), simple] {
                subtypes_by_super_name
                    .entry(key)
                    .or_default()
                    .push(edge.subtype.clone());
            }
        }
        for subtypes in subtypes_by_super_name.values_mut() {
            subtypes.sort();
            subtypes.dedup();
        }

        Self {
            symbols,
            by_id,
            type_by_qualified,
            type_by_simple,
            methods_by_owner_name_arity,
            methods_by_name_arity,
            subtypes_by_super_name,
            type_edges,
        }
    }

    fn symbol(&self, symbol_id: &str) -> Option<&JavaSymbol> {
        self.by_id.get(symbol_id)
    }

    fn resolve_type(&self, type_name: &str, package: Option<&str>) -> Option<String> {
        let erased = erase_type(type_name);
        if let Some(symbol) = self.type_by_qualified.get(&erased) {
            return Some(symbol.clone());
        }
        if let Some(package) = package {
            let qualified = format!("{package}.{erased}");
            if let Some(symbol) = self.type_by_qualified.get(&qualified) {
                return Some(symbol.clone());
            }
        }
        self.type_by_simple
            .get(&last_identifier(&erased))
            .and_then(|items| (items.len() == 1).then(|| items[0].clone()))
    }

    fn owner_type_for_method(&self, method_id: &str) -> Option<&JavaSymbol> {
        let method = self.symbol(method_id)?;
        let owner = method.owner_symbol.as_ref()?;
        self.symbol(owner)
    }

    fn methods_on_type(&self, type_id: &str, name: &str, arity: usize) -> Vec<String> {
        self.methods_by_owner_name_arity
            .get(&(type_id.to_string(), name.to_string(), arity))
            .cloned()
            .unwrap_or_default()
    }

    fn global_methods(&self, name: &str, arity: usize) -> Vec<String> {
        self.methods_by_name_arity
            .get(&(name.to_string(), arity))
            .cloned()
            .unwrap_or_default()
    }

    fn override_candidates(&self, method_id: &str) -> Vec<String> {
        let Some(method) = self.symbol(method_id) else {
            return Vec::new();
        };
        let Some(owner) = method.owner_symbol.as_deref() else {
            return Vec::new();
        };
        let mut subtypes = self.direct_and_transitive_subtypes(owner);
        subtypes.sort();
        subtypes.dedup();
        let lookup_name = method_lookup_name(method);
        let mut candidates = Vec::new();
        for subtype in subtypes {
            candidates.extend(self.methods_on_type(
                &subtype,
                &lookup_name,
                method.parameters.len(),
            ));
        }
        candidates
    }

    fn direct_and_transitive_subtypes(&self, type_id: &str) -> Vec<String> {
        let Some(type_symbol) = self.symbol(type_id) else {
            return Vec::new();
        };
        let mut resolved = Vec::new();
        let mut queue = vec![type_symbol.qualified_name.clone(), type_symbol.name.clone()];
        let mut seen = BTreeSet::new();
        while let Some(super_name) = queue.pop() {
            if !seen.insert(super_name.clone()) {
                continue;
            }
            let Some(edges) = self.subtypes_by_super_name.get(&super_name) else {
                continue;
            };
            for edge_subtype in edges {
                let subtype = if self.symbol(edge_subtype).is_some() {
                    Some(edge_subtype.clone())
                } else {
                    self.resolve_type(edge_subtype, None)
                };
                if let Some(subtype) = subtype {
                    if !resolved.contains(&subtype) {
                        if let Some(symbol) = self.symbol(&subtype) {
                            queue.push(symbol.qualified_name.clone());
                            queue.push(symbol.name.clone());
                        }
                        resolved.push(subtype);
                    }
                }
            }
        }
        resolved
    }
}

fn resolve_call(tables: &ResolverTables, raw: RawJavaCall) -> JavaCallEdge {
    let caller_owner = tables.owner_type_for_method(&raw.caller_symbol);
    let caller_package = caller_owner.map(|owner| owner.package.as_str());
    let mut candidates = match raw.dispatch_kind {
        DispatchKind::Constructor => raw
            .receiver_type
            .as_deref()
            .and_then(|ty| tables.resolve_type(ty, caller_package))
            .map(|type_id| tables.methods_on_type(&type_id, "<init>", raw.arg_count))
            .unwrap_or_default(),
        DispatchKind::Static | DispatchKind::Virtual | DispatchKind::Interface => {
            if let Some(receiver_type) = raw.receiver_type.as_deref() {
                tables
                    .resolve_type(receiver_type, caller_package)
                    .map(|type_id| {
                        tables.methods_on_type(&type_id, &raw.target_name, raw.arg_count)
                    })
                    .unwrap_or_default()
            } else if let Some(owner) = caller_owner {
                let mut local =
                    tables.methods_on_type(&owner.symbol_id, &raw.target_name, raw.arg_count);
                if local.is_empty() {
                    local = tables.global_methods(&raw.target_name, raw.arg_count);
                }
                local
            } else {
                tables.global_methods(&raw.target_name, raw.arg_count)
            }
        }
        DispatchKind::Super => caller_owner
            .and_then(|owner| {
                tables
                    .type_edges
                    .iter()
                    .find(|edge| {
                        edge.subtype == owner.symbol_id || edge.subtype == owner.qualified_name
                    })
                    .and_then(|edge| tables.resolve_type(&edge.supertype, caller_package))
            })
            .map(|type_id| tables.methods_on_type(&type_id, &raw.target_name, raw.arg_count))
            .unwrap_or_default(),
        DispatchKind::MethodReference | DispatchKind::Unknown => {
            tables.global_methods(&raw.target_name, raw.arg_count)
        }
    };

    candidates.sort();
    candidates.dedup();
    let (callee_symbol, status, confidence) = match candidates.len() {
        0 => (
            None,
            ResolveStatus::Unresolved,
            ResolveConfidence::Unresolved,
        ),
        1 => (
            Some(candidates[0].clone()),
            ResolveStatus::Resolved,
            candidate_confidence(tables, &candidates[0]),
        ),
        _ => (None, ResolveStatus::Ambiguous, ResolveConfidence::Ambiguous),
    };
    let possible_callees = callee_symbol
        .as_deref()
        .map(|symbol_id| {
            let mut possible = vec![symbol_id.to_string()];
            if matches!(
                raw.dispatch_kind,
                DispatchKind::Virtual | DispatchKind::Interface | DispatchKind::Super
            ) {
                possible.extend(tables.override_candidates(symbol_id));
            }
            possible.sort();
            possible.dedup();
            possible
        })
        .unwrap_or_else(|| candidates.clone());

    JavaCallEdge {
        caller_symbol: raw.caller_symbol,
        callee_symbol,
        possible_callees,
        target_name: raw.target_name,
        path: raw.path,
        range: raw.range,
        file_hash: raw.file_hash,
        dispatch_kind: raw.dispatch_kind,
        receiver_type: raw.receiver_type,
        status,
        confidence,
    }
}

fn method_lookup_name(symbol: &JavaSymbol) -> String {
    if symbol.kind == JavaSymbolKind::Constructor {
        "<init>".to_string()
    } else {
        symbol.name.clone()
    }
}

fn candidate_confidence(tables: &ResolverTables, symbol_id: &str) -> ResolveConfidence {
    tables
        .symbol(symbol_id)
        .map(|symbol| symbol.confidence)
        .unwrap_or(ResolveConfidence::Unresolved)
}
