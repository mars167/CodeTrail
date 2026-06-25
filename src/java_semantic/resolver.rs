use std::collections::{BTreeMap, BTreeSet};

use crate::java_semantic::{
    model::{
        DispatchKind, ExtractedJavaFile, JavaCallEdge, JavaFileContribution, JavaOccurrence,
        JavaSemanticData, JavaSemanticManifest, JavaSymbol, JavaSymbolKind, OccurrenceRole,
        RawJavaArgumentCall, RawJavaCall, ResolveConfidence, ResolveStatus,
    },
    parse::{erase_type, last_identifier},
};

#[derive(Clone, Debug)]
pub struct ResolverInput {
    pub manifest: JavaSemanticManifest,
    pub files: Vec<ExtractedJavaFile>,
    pub external_symbols: Vec<JavaSymbol>,
    pub external_type_edges: Vec<crate::java_semantic::model::JavaTypeEdge>,
}

#[derive(Clone, Debug)]
struct ResolverTables {
    symbols: Vec<JavaSymbol>,
    by_id: BTreeMap<String, JavaSymbol>,
    type_by_qualified: BTreeMap<String, String>,
    type_by_simple: BTreeMap<String, Vec<String>>,
    methods_by_owner_name_arity: BTreeMap<(String, String, usize), Vec<String>>,
    methods_by_name_arity: BTreeMap<(String, usize), Vec<String>>,
    supertypes_by_subtype: BTreeMap<String, Vec<String>>,
    subtypes_by_super_name: BTreeMap<String, Vec<String>>,
    type_edges: Vec<crate::java_semantic::model::JavaTypeEdge>,
}

pub fn resolve(input: ResolverInput) -> JavaSemanticData {
    let mut symbols = input.external_symbols;
    let mut occurrences = Vec::new();
    let mut raw_calls = Vec::new();
    let mut type_edges = input.external_type_edges;
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
        let mut supertypes_by_subtype = BTreeMap::<String, Vec<String>>::new();
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
            supertypes_by_subtype
                .entry(edge.subtype.clone())
                .or_default()
                .push(edge.supertype.clone());
            let simple = last_identifier(&edge.supertype);
            for key in [edge.supertype.clone(), simple] {
                subtypes_by_super_name
                    .entry(key)
                    .or_default()
                    .push(edge.subtype.clone());
            }
        }
        for supertypes in supertypes_by_subtype.values_mut() {
            supertypes.sort();
            supertypes.dedup();
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
            supertypes_by_subtype,
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
        let java_lang = format!("java.lang.{}", last_identifier(&erased));
        if let Some(symbol) = self.type_by_qualified.get(&java_lang) {
            return Some(symbol.clone());
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

    fn methods_on_type_or_supers(&self, type_id: &str, name: &str, arity: usize) -> Vec<String> {
        let direct = self.methods_on_type(type_id, name, arity);
        if !direct.is_empty() {
            return direct;
        }
        let mut candidates = Vec::new();
        let mut queue = self
            .supertypes_by_subtype
            .get(type_id)
            .into_iter()
            .flatten()
            .filter_map(|supertype| self.resolve_type(supertype, None))
            .collect::<Vec<_>>();
        let mut seen = BTreeSet::new();
        while let Some(current) = queue.pop() {
            if !seen.insert(current.clone()) {
                continue;
            }
            candidates.extend(self.methods_on_type(&current, name, arity));
            let Some(supertypes) = self.supertypes_by_subtype.get(&current) else {
                continue;
            };
            for supertype in supertypes {
                if let Some(resolved) = self.resolve_type(supertype, None) {
                    queue.push(resolved);
                }
            }
        }
        candidates.sort();
        candidates.dedup();
        candidates
    }

    fn is_assignable_to(&self, arg_type: &str, parameter_type: &str) -> bool {
        let Some(arg_type) = self.resolve_type(arg_type, None) else {
            return false;
        };
        let Some(parameter_type) = self.resolve_type(parameter_type, None) else {
            return false;
        };
        if arg_type == parameter_type {
            return true;
        }
        let mut queue = vec![arg_type];
        let mut seen = BTreeSet::new();
        while let Some(current) = queue.pop() {
            if !seen.insert(current.clone()) {
                continue;
            }
            let Some(supertypes) = self.supertypes_by_subtype.get(&current) else {
                continue;
            };
            for supertype in supertypes {
                let Some(resolved) = self.resolve_type(supertype, None) else {
                    continue;
                };
                if resolved == parameter_type {
                    return true;
                }
                queue.push(resolved);
            }
        }
        false
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
                        tables.methods_on_type_or_supers(&type_id, &raw.target_name, raw.arg_count)
                    })
                    .unwrap_or_default()
            } else if raw.receiver_text.is_some() {
                Vec::new()
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
    let arg_types = enriched_arg_types(tables, &raw);
    candidates = disambiguate_by_argument_types(tables, candidates, &arg_types);
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

fn enriched_arg_types(tables: &ResolverTables, raw: &RawJavaCall) -> Vec<Option<String>> {
    raw.arg_types
        .iter()
        .enumerate()
        .map(|(idx, arg_type)| {
            arg_type.clone().or_else(|| {
                raw.arg_calls
                    .get(idx)
                    .and_then(Option::as_ref)
                    .and_then(|call| infer_argument_call_return_type(tables, call))
            })
        })
        .collect()
}

fn infer_argument_call_return_type(
    tables: &ResolverTables,
    call: &RawJavaArgumentCall,
) -> Option<String> {
    let mut candidates = match call.dispatch_kind {
        DispatchKind::Constructor => return call.receiver_type.clone(),
        DispatchKind::Static | DispatchKind::Virtual | DispatchKind::Interface => call
            .receiver_type
            .as_deref()
            .and_then(|receiver_type| tables.resolve_type(receiver_type, None))
            .map(|type_id| {
                tables.methods_on_type_or_supers(&type_id, &call.target_name, call.arg_count)
            })
            .unwrap_or_default(),
        DispatchKind::Super | DispatchKind::MethodReference | DispatchKind::Unknown => {
            tables.global_methods(&call.target_name, call.arg_count)
        }
    };
    candidates.sort();
    candidates.dedup();
    let candidates = disambiguate_by_argument_types(tables, candidates, &[]);
    if candidates.len() != 1 {
        return None;
    }
    tables
        .symbol(&candidates[0])
        .and_then(|symbol| symbol.return_type.clone())
}

fn disambiguate_by_argument_types(
    tables: &ResolverTables,
    candidates: Vec<String>,
    arg_types: &[Option<String>],
) -> Vec<String> {
    if candidates.len() <= 1 {
        return candidates;
    }
    if arg_types.iter().all(Option::is_none) {
        return prefer_reference_overload(tables, candidates);
    }
    let mut scored = candidates
        .iter()
        .filter_map(|candidate| {
            tables
                .symbol(candidate)
                .and_then(|symbol| parameters_match_score(tables, &symbol.parameters, arg_types))
                .map(|score| (score, candidate.clone()))
        })
        .collect::<Vec<_>>();
    if scored.is_empty() {
        candidates
    } else {
        scored.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        let best = scored[0].0;
        scored
            .into_iter()
            .filter_map(|(score, candidate)| (score == best).then_some(candidate))
            .collect()
    }
}

fn prefer_reference_overload(tables: &ResolverTables, candidates: Vec<String>) -> Vec<String> {
    let filtered = candidates
        .iter()
        .filter(|candidate| {
            tables.symbol(candidate).is_some_and(|symbol| {
                symbol
                    .parameters
                    .iter()
                    .all(|parameter| !is_primitive_like(parameter))
            })
        })
        .cloned()
        .collect::<Vec<_>>();
    if filtered.len() == 1 {
        filtered
    } else {
        candidates
    }
}

fn parameters_match_score(
    tables: &ResolverTables,
    parameters: &[String],
    arg_types: &[Option<String>],
) -> Option<u32> {
    let mut total = 0;
    for (idx, arg_type) in arg_types.iter().enumerate() {
        let Some(arg_type) = arg_type else {
            total += 5;
            continue;
        };
        let Some(parameter) = parameters.get(idx) else {
            return None;
        };
        total += parameter_match_score(tables, parameter, arg_type)?;
    }
    Some(total)
}

fn parameter_match_score(tables: &ResolverTables, parameter: &str, arg_type: &str) -> Option<u32> {
    let parameter = normalize_match_type(parameter);
    let arg_type = normalize_match_type(arg_type);
    if parameter == "Object" || parameter == "java.lang.Object" {
        return Some(10);
    }
    if parameter == arg_type || last_identifier(&parameter) == last_identifier(&arg_type) {
        Some(0)
    } else if tables.is_assignable_to(&arg_type, &parameter) {
        Some(1)
    } else {
        None
    }
}

fn normalize_match_type(value: &str) -> String {
    let value = value.trim();
    if let Some(base) = value.strip_suffix("...") {
        format!("{base}[]")
    } else {
        value.to_string()
    }
}

fn is_primitive_like(value: &str) -> bool {
    let value = normalize_match_type(value);
    let base = value.trim_end_matches("[]");
    matches!(
        base,
        "boolean" | "byte" | "char" | "double" | "float" | "int" | "long" | "short"
    )
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
