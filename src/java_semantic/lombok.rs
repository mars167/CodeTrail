use std::collections::{BTreeMap, BTreeSet};

use crate::java_semantic::{
    extract::{field_symbol_id, method_symbol_id},
    model::{
        ExtractedJavaFile, JavaAnnotation, JavaSymbol, JavaSymbolKind, ResolveConfidence,
        SymbolOrigin,
    },
};

pub fn apply_lombok_overlay(file: &mut ExtractedJavaFile) {
    if file.annotations.is_empty() {
        return;
    }
    let annotations = file.annotations.clone();
    let symbols = file.symbols.clone();
    let by_id = symbols
        .iter()
        .map(|symbol| (symbol.symbol_id.clone(), symbol.clone()))
        .collect::<BTreeMap<_, _>>();
    let fields_by_owner = fields_by_owner(&symbols);
    let mut seen = file
        .symbols
        .iter()
        .map(|symbol| symbol.symbol_id.clone())
        .collect::<BTreeSet<_>>();

    for annotation in annotations {
        let normalized = annotation_name(&annotation);
        let Some(owner) = by_id.get(&annotation.owner_symbol) else {
            continue;
        };
        match owner.kind {
            JavaSymbolKind::Type | JavaSymbolKind::Annotation => {
                let fields = fields_by_owner
                    .get(&owner.symbol_id)
                    .cloned()
                    .unwrap_or_default();
                match normalized.as_str() {
                    "Getter" => add_getters(file, owner, &fields, &mut seen),
                    "Setter" => add_setters(file, owner, &fields, &mut seen),
                    "Data" | "Value" => {
                        add_getters(file, owner, &fields, &mut seen);
                        if normalized == "Data" {
                            add_setters(file, owner, &fields, &mut seen);
                        }
                    }
                    "Builder" => add_builder(file, owner, &mut seen),
                    "NoArgsConstructor" => add_constructor(file, owner, Vec::new(), &mut seen),
                    "AllArgsConstructor" => {
                        let parameters = fields
                            .iter()
                            .map(|field| {
                                field
                                    .return_type
                                    .clone()
                                    .unwrap_or_else(|| "Object".to_string())
                            })
                            .collect();
                        add_constructor(file, owner, parameters, &mut seen);
                    }
                    "RequiredArgsConstructor" => {
                        let parameters = fields
                            .iter()
                            .filter(|field| {
                                field.modifiers.iter().any(|m| m == "final")
                                    || field.modifiers.iter().any(|m| m.ends_with("NonNull"))
                            })
                            .map(|field| {
                                field
                                    .return_type
                                    .clone()
                                    .unwrap_or_else(|| "Object".to_string())
                            })
                            .collect();
                        add_constructor(file, owner, parameters, &mut seen);
                    }
                    "Slf4j" => add_log_field(file, owner, &mut seen),
                    _ => {}
                }
            }
            JavaSymbolKind::Field => {
                let Some(type_owner_id) = owner.owner_symbol.as_deref() else {
                    continue;
                };
                let Some(type_owner) = by_id.get(type_owner_id) else {
                    continue;
                };
                match normalized.as_str() {
                    "Getter" => {
                        add_getters(file, type_owner, std::slice::from_ref(owner), &mut seen)
                    }
                    "Setter" => {
                        add_setters(file, type_owner, std::slice::from_ref(owner), &mut seen)
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

fn annotation_name(annotation: &JavaAnnotation) -> String {
    annotation
        .name
        .rsplit('.')
        .next()
        .unwrap_or(&annotation.name)
        .trim_start_matches('@')
        .to_string()
}

fn fields_by_owner(symbols: &[JavaSymbol]) -> BTreeMap<String, Vec<JavaSymbol>> {
    let mut grouped = BTreeMap::<String, Vec<JavaSymbol>>::new();
    for symbol in symbols {
        if symbol.kind == JavaSymbolKind::Field {
            if let Some(owner) = &symbol.owner_symbol {
                grouped
                    .entry(owner.clone())
                    .or_default()
                    .push(symbol.clone());
            }
        }
    }
    grouped
}

fn add_getters(
    file: &mut ExtractedJavaFile,
    owner: &JavaSymbol,
    fields: &[JavaSymbol],
    seen: &mut BTreeSet<String>,
) {
    for field in fields {
        let prefix = if field.return_type.as_deref() == Some("boolean") {
            "is"
        } else {
            "get"
        };
        add_method(
            file,
            owner,
            &format!("{prefix}{}", capitalized(&field.name)),
            Vec::new(),
            field.return_type.clone(),
            seen,
        );
    }
}

fn add_setters(
    file: &mut ExtractedJavaFile,
    owner: &JavaSymbol,
    fields: &[JavaSymbol],
    seen: &mut BTreeSet<String>,
) {
    for field in fields {
        add_method(
            file,
            owner,
            &format!("set{}", capitalized(&field.name)),
            vec![field
                .return_type
                .clone()
                .unwrap_or_else(|| "Object".to_string())],
            Some("void".to_string()),
            seen,
        );
    }
}

fn add_builder(file: &mut ExtractedJavaFile, owner: &JavaSymbol, seen: &mut BTreeSet<String>) {
    add_method(
        file,
        owner,
        "builder",
        Vec::new(),
        Some(format!("{}Builder", owner.name)),
        seen,
    );
}

fn add_constructor(
    file: &mut ExtractedJavaFile,
    owner: &JavaSymbol,
    parameters: Vec<String>,
    seen: &mut BTreeSet<String>,
) {
    let symbol_id = method_symbol_id(&owner.root_id, &owner.qualified_name, "<init>", &parameters);
    if !seen.insert(symbol_id.clone()) {
        return;
    }
    file.symbols.push(JavaSymbol {
        symbol_id,
        name: owner.name.clone(),
        kind: JavaSymbolKind::Constructor,
        package: owner.package.clone(),
        qualified_name: format!("{}#<init>", owner.qualified_name),
        owner_symbol: Some(owner.symbol_id.clone()),
        path: owner.path.clone(),
        range: owner.range.clone(),
        selection_range: owner.selection_range.clone(),
        descriptor: Some(format!("({})", parameters.join(","))),
        parameters,
        return_type: None,
        modifiers: vec!["lombok_synthetic".to_string()],
        origin: SymbolOrigin::LombokSynthetic,
        confidence: ResolveConfidence::SyntheticAnnotationModel,
        root_id: owner.root_id.clone(),
        file_hash: owner.file_hash.clone(),
    });
}

fn add_method(
    file: &mut ExtractedJavaFile,
    owner: &JavaSymbol,
    name: &str,
    parameters: Vec<String>,
    return_type: Option<String>,
    seen: &mut BTreeSet<String>,
) {
    let symbol_id = method_symbol_id(&owner.root_id, &owner.qualified_name, name, &parameters);
    if !seen.insert(symbol_id.clone()) {
        return;
    }
    file.symbols.push(JavaSymbol {
        symbol_id,
        name: name.to_string(),
        kind: JavaSymbolKind::SyntheticMethod,
        package: owner.package.clone(),
        qualified_name: format!("{}#{}", owner.qualified_name, name),
        owner_symbol: Some(owner.symbol_id.clone()),
        path: owner.path.clone(),
        range: owner.range.clone(),
        selection_range: owner.selection_range.clone(),
        descriptor: Some(format!("({})", parameters.join(","))),
        parameters,
        return_type,
        modifiers: vec!["lombok_synthetic".to_string()],
        origin: SymbolOrigin::LombokSynthetic,
        confidence: ResolveConfidence::SyntheticAnnotationModel,
        root_id: owner.root_id.clone(),
        file_hash: owner.file_hash.clone(),
    });
}

fn add_log_field(file: &mut ExtractedJavaFile, owner: &JavaSymbol, seen: &mut BTreeSet<String>) {
    let symbol_id = field_symbol_id(&owner.root_id, &owner.qualified_name, "log");
    if !seen.insert(symbol_id.clone()) {
        return;
    }
    file.symbols.push(JavaSymbol {
        symbol_id,
        name: "log".to_string(),
        kind: JavaSymbolKind::Field,
        package: owner.package.clone(),
        qualified_name: format!("{}#log", owner.qualified_name),
        owner_symbol: Some(owner.symbol_id.clone()),
        path: owner.path.clone(),
        range: owner.range.clone(),
        selection_range: owner.selection_range.clone(),
        descriptor: None,
        parameters: Vec::new(),
        return_type: Some("org.slf4j.Logger".to_string()),
        modifiers: vec![
            "private".to_string(),
            "static".to_string(),
            "final".to_string(),
            "lombok_synthetic".to_string(),
        ],
        origin: SymbolOrigin::LombokSynthetic,
        confidence: ResolveConfidence::SyntheticAnnotationModel,
        root_id: owner.root_id.clone(),
        file_hash: owner.file_hash.clone(),
    });
}

fn capitalized(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}
