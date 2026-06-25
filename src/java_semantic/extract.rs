use std::{collections::BTreeMap, fs};

use anyhow::{Context, Result};
use tree_sitter::{Node, Parser};

use crate::{
    java_semantic::{
        model::{
            DispatchKind, ExtractedJavaFile, JavaAnnotation, JavaImport, JavaSymbol,
            JavaSymbolKind, JavaTypeEdge, RawJavaCall, ResolveConfidence, SymbolOrigin,
        },
        parse::{
            child_by_kind, child_text, erase_type, last_identifier, named_children, node_text,
            point_range,
        },
    },
    workspace::{FileRecord, Workspace},
};

#[derive(Clone, Debug)]
struct TypeContext {
    symbol_id: String,
    name: String,
    qualified_name: String,
    package: String,
}

#[derive(Clone, Debug)]
struct MethodContext<'tree> {
    symbol: JavaSymbol,
    body: Option<Node<'tree>>,
    parameter_types: BTreeMap<String, String>,
}

pub fn extract_file(
    workspace: &Workspace,
    file: &FileRecord,
    root_id: &str,
    generated: bool,
) -> Result<ExtractedJavaFile> {
    let path = workspace.abs_path(&file.path);
    let source = fs::read_to_string(&path)
        .with_context(|| format!("failed to read Java source {}", file.path))?;
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_java::LANGUAGE.into())?;
    let tree = parser
        .parse(&source, None)
        .with_context(|| format!("tree-sitter failed to parse {}", file.path))?;

    let source_bytes = source.as_bytes();
    let root = tree.root_node();

    let package = package_name(root, source_bytes).unwrap_or_default();
    let imports = imports(root, source_bytes);
    let mut out = ExtractedJavaFile {
        path: file.path.clone(),
        root_id: root_id.to_string(),
        file_hash: file.hash.clone(),
        package: package.clone(),
        imports,
        symbols: Vec::new(),
        raw_calls: Vec::new(),
        type_edges: Vec::new(),
        annotations: Vec::new(),
        generated,
    };

    let mut methods = Vec::new();
    let mut type_stack = Vec::new();
    walk_top_level(
        root,
        source_bytes,
        &mut out,
        &mut methods,
        &mut type_stack,
        &package,
        root_id,
        generated,
    );

    for method in methods {
        if let Some(body) = method.body {
            let mut local_types = method.parameter_types;
            local_types.extend(local_variable_types(body, source_bytes));
            collect_calls_in_method(
                body,
                source_bytes,
                &mut out.raw_calls,
                &method.symbol,
                &local_types,
                &out.imports,
                &out.package,
            );
        }
    }

    Ok(out)
}

fn walk_top_level<'tree>(
    node: Node<'tree>,
    source: &[u8],
    out: &mut ExtractedJavaFile,
    methods: &mut Vec<MethodContext<'tree>>,
    type_stack: &mut Vec<TypeContext>,
    package: &str,
    root_id: &str,
    generated: bool,
) {
    if let Some(type_kind) = type_kind(node.kind()) {
        let Some(name_node) = node.child_by_field_name("name") else {
            return;
        };
        let Some(name) = node_text(name_node, source) else {
            return;
        };
        let owner = type_stack.last();
        let qualified_name = if let Some(owner) = owner {
            format!("{}.{}", owner.qualified_name, name)
        } else if package.is_empty() {
            name.clone()
        } else {
            format!("{package}.{name}")
        };
        let symbol_id = type_symbol_id(root_id, &qualified_name);
        let origin = if generated {
            SymbolOrigin::GeneratedSource
        } else {
            SymbolOrigin::Source
        };
        let symbol = JavaSymbol {
            symbol_id: symbol_id.clone(),
            name: name.clone(),
            kind: type_kind,
            package: package.to_string(),
            qualified_name: qualified_name.clone(),
            owner_symbol: owner.map(|owner| owner.symbol_id.clone()),
            path: Some(out.path.clone()),
            range: Some(point_range(node)),
            selection_range: Some(point_range(name_node)),
            descriptor: None,
            parameters: Vec::new(),
            return_type: None,
            modifiers: modifiers(node, source),
            origin,
            confidence: if generated {
                ResolveConfidence::GeneratedSource
            } else {
                ResolveConfidence::SourceResolver
            },
            root_id: root_id.to_string(),
            file_hash: out.file_hash.clone(),
        };
        let annotations = annotations(node, source);
        out.annotations
            .extend(annotations.into_iter().map(|name| JavaAnnotation {
                name,
                owner_symbol: symbol_id.clone(),
            }));
        let supers = super_types(node, source);
        for supertype in &supers {
            out.type_edges.push(JavaTypeEdge {
                subtype: symbol_id.clone(),
                supertype: supertype.clone(),
                relation: "extends_or_implements".to_string(),
            });
        }
        out.symbols.push(symbol);
        type_stack.push(TypeContext {
            symbol_id,
            name,
            qualified_name,
            package: package.to_string(),
        });
        walk_children(
            node, source, out, methods, type_stack, package, root_id, generated,
        );
        type_stack.pop();
        return;
    }

    if is_method_node(node.kind()) {
        if let Some(symbol) = method_symbol(node, source, out, type_stack, root_id, generated) {
            let annotations = annotations(node, source);
            out.annotations
                .extend(annotations.into_iter().map(|name| JavaAnnotation {
                    name,
                    owner_symbol: symbol.symbol_id.clone(),
                }));
            let body = node.child_by_field_name("body");
            methods.push(MethodContext {
                symbol: symbol.clone(),
                body,
                parameter_types: parameter_type_bindings(node, source),
            });
            out.symbols.push(symbol);
        }
        return;
    }

    if node.kind() == "field_declaration" {
        field_symbols(node, source, out, type_stack, root_id, generated);
        return;
    }

    walk_children(
        node, source, out, methods, type_stack, package, root_id, generated,
    );
}

fn walk_children<'tree>(
    node: Node<'tree>,
    source: &[u8],
    out: &mut ExtractedJavaFile,
    methods: &mut Vec<MethodContext<'tree>>,
    type_stack: &mut Vec<TypeContext>,
    package: &str,
    root_id: &str,
    generated: bool,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk_top_level(
            child, source, out, methods, type_stack, package, root_id, generated,
        );
    }
}

fn package_name(root: Node, source: &[u8]) -> Option<String> {
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        if child.kind() == "package_declaration" {
            return child
                .child_by_field_name("name")
                .or_else(|| child_by_kind(child, "scoped_identifier"))
                .and_then(|name| node_text(name, source));
        }
    }
    None
}

fn imports(root: Node, source: &[u8]) -> Vec<JavaImport> {
    let mut imports = Vec::new();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        if child.kind() != "import_declaration" {
            continue;
        }
        let text = node_text(child, source).unwrap_or_default();
        let is_static = text.contains(" static ");
        let is_wildcard = text.contains(".*");
        let path = text
            .trim_start_matches("import")
            .trim()
            .trim_start_matches("static")
            .trim()
            .trim_end_matches(';')
            .trim()
            .trim_end_matches(".*")
            .to_string();
        if !path.is_empty() {
            imports.push(JavaImport {
                path,
                is_static,
                is_wildcard,
            });
        }
    }
    imports
}

fn type_kind(kind: &str) -> Option<JavaSymbolKind> {
    match kind {
        "class_declaration"
        | "interface_declaration"
        | "enum_declaration"
        | "record_declaration" => Some(JavaSymbolKind::Type),
        "annotation_type_declaration" => Some(JavaSymbolKind::Annotation),
        _ => None,
    }
}

fn is_method_node(kind: &str) -> bool {
    matches!(kind, "method_declaration" | "constructor_declaration")
}

fn method_symbol(
    node: Node,
    source: &[u8],
    out: &ExtractedJavaFile,
    type_stack: &[TypeContext],
    root_id: &str,
    generated: bool,
) -> Option<JavaSymbol> {
    let owner = type_stack.last()?;
    let name_node = node.child_by_field_name("name")?;
    let raw_name = node_text(name_node, source)?;
    let constructor = node.kind() == "constructor_declaration";
    let name = if constructor {
        owner.name.clone()
    } else {
        raw_name
    };
    let parameters = parameter_types(node, source);
    let return_type = (!constructor)
        .then(|| child_text(node, "type", source).unwrap_or_else(|| "void".to_string()))
        .map(|value| erase_type(&value));
    let symbol_id = method_symbol_id(
        root_id,
        &owner.qualified_name,
        if constructor { "<init>" } else { &name },
        parameters.len(),
    );
    let origin = if generated {
        SymbolOrigin::GeneratedSource
    } else {
        SymbolOrigin::Source
    };
    Some(JavaSymbol {
        symbol_id,
        name,
        kind: if constructor {
            JavaSymbolKind::Constructor
        } else {
            JavaSymbolKind::Method
        },
        package: owner.package.clone(),
        qualified_name: format!("{}#{}", owner.qualified_name, name_node_text(node, source)?),
        owner_symbol: Some(owner.symbol_id.clone()),
        path: Some(out.path.clone()),
        range: Some(point_range(node)),
        selection_range: Some(point_range(name_node)),
        descriptor: Some(format!("({})", parameters.join(","))),
        parameters,
        return_type,
        modifiers: modifiers(node, source),
        origin,
        confidence: if generated {
            ResolveConfidence::GeneratedSource
        } else {
            ResolveConfidence::SourceResolver
        },
        root_id: root_id.to_string(),
        file_hash: out.file_hash.clone(),
    })
}

fn name_node_text(node: Node, source: &[u8]) -> Option<String> {
    node.child_by_field_name("name")
        .and_then(|name| node_text(name, source))
}

fn field_symbols(
    node: Node,
    source: &[u8],
    out: &mut ExtractedJavaFile,
    type_stack: &[TypeContext],
    root_id: &str,
    generated: bool,
) {
    let Some(owner) = type_stack.last() else {
        return;
    };
    let field_type = child_text(node, "type", source).map(|value| erase_type(&value));
    for declarator in descendants(node).filter(|child| child.kind() == "variable_declarator") {
        let Some(name_node) = declarator.child_by_field_name("name") else {
            continue;
        };
        let Some(name) = node_text(name_node, source) else {
            continue;
        };
        let symbol_id = field_symbol_id(root_id, &owner.qualified_name, &name);
        let origin = if generated {
            SymbolOrigin::GeneratedSource
        } else {
            SymbolOrigin::Source
        };
        out.symbols.push(JavaSymbol {
            symbol_id: symbol_id.clone(),
            name: name.clone(),
            kind: JavaSymbolKind::Field,
            package: owner.package.clone(),
            qualified_name: format!("{}#{}", owner.qualified_name, name),
            owner_symbol: Some(owner.symbol_id.clone()),
            path: Some(out.path.clone()),
            range: Some(point_range(node)),
            selection_range: Some(point_range(name_node)),
            descriptor: None,
            parameters: Vec::new(),
            return_type: field_type.clone(),
            modifiers: modifiers(node, source),
            origin,
            confidence: if generated {
                ResolveConfidence::GeneratedSource
            } else {
                ResolveConfidence::SourceResolver
            },
            root_id: root_id.to_string(),
            file_hash: out.file_hash.clone(),
        });
        for annotation in annotations(node, source) {
            out.annotations.push(JavaAnnotation {
                name: annotation,
                owner_symbol: symbol_id.clone(),
            });
        }
    }
}

fn parameter_types(node: Node, source: &[u8]) -> Vec<String> {
    let Some(parameters) = node.child_by_field_name("parameters") else {
        return Vec::new();
    };
    named_children(parameters)
        .into_iter()
        .filter(|child| {
            matches!(
                child.kind(),
                "formal_parameter" | "spread_parameter" | "receiver_parameter"
            )
        })
        .filter_map(|parameter| child_text(parameter, "type", source))
        .map(|value| erase_type(&value))
        .collect()
}

fn parameter_type_bindings(node: Node, source: &[u8]) -> BTreeMap<String, String> {
    let Some(parameters) = node.child_by_field_name("parameters") else {
        return BTreeMap::new();
    };
    let mut types = BTreeMap::new();
    for parameter in named_children(parameters).into_iter().filter(|child| {
        matches!(
            child.kind(),
            "formal_parameter" | "spread_parameter" | "receiver_parameter"
        )
    }) {
        let Some(name) = child_text(parameter, "name", source) else {
            continue;
        };
        let Some(type_name) = child_text(parameter, "type", source) else {
            continue;
        };
        types.insert(name, erase_type(&type_name));
    }
    types
}

fn modifiers(node: Node, source: &[u8]) -> Vec<String> {
    node.child_by_field_name("modifiers")
        .or_else(|| child_by_kind(node, "modifiers"))
        .map(|mods| {
            named_children(mods)
                .into_iter()
                .filter_map(|child| node_text(child, source))
                .map(|value| value.trim_start_matches('@').to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn annotations(node: Node, source: &[u8]) -> Vec<String> {
    node.child_by_field_name("modifiers")
        .or_else(|| child_by_kind(node, "modifiers"))
        .map(|mods| {
            named_children(mods)
                .into_iter()
                .filter(|child| {
                    child.kind().contains("annotation")
                        || node_text(*child, source).is_some_and(|text| text.starts_with('@'))
                })
                .filter_map(|child| node_text(child, source))
                .map(|text| {
                    let text = text.trim_start_matches('@');
                    text.split(['(', ' ', '\n', '\t'])
                        .next()
                        .unwrap_or(text)
                        .to_string()
                })
                .filter(|name| !name.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn super_types(node: Node, source: &[u8]) -> Vec<String> {
    let mut supers = Vec::new();
    for field in ["superclass", "interfaces", "super_interfaces"] {
        if let Some(child) = node.child_by_field_name(field) {
            for item in descendants(child) {
                if matches!(
                    item.kind(),
                    "type_identifier" | "scoped_type_identifier" | "generic_type"
                ) {
                    if let Some(text) = node_text(item, source) {
                        supers.push(erase_type(&text));
                    }
                }
            }
        }
    }
    supers.sort();
    supers.dedup();
    supers
}

fn local_variable_types(body: Node, source: &[u8]) -> BTreeMap<String, String> {
    let mut types = BTreeMap::new();
    for node in descendants(body) {
        if !matches!(
            node.kind(),
            "local_variable_declaration" | "field_declaration" | "variable_declaration"
        ) {
            continue;
        }
        let Some(type_text) = child_text(node, "type", source).map(|value| erase_type(&value))
        else {
            continue;
        };
        for declarator in descendants(node).filter(|child| child.kind() == "variable_declarator") {
            if let Some(name) = declarator
                .child_by_field_name("name")
                .and_then(|name| node_text(name, source))
            {
                types.insert(name, type_text.clone());
            }
        }
    }
    types
}

fn collect_calls_in_method(
    body: Node,
    source: &[u8],
    calls: &mut Vec<RawJavaCall>,
    caller: &JavaSymbol,
    local_types: &BTreeMap<String, String>,
    imports: &[JavaImport],
    package: &str,
) {
    for node in descendants(body) {
        let Some((target_name, receiver_text, arg_count, dispatch_kind)) =
            call_target(node, source)
        else {
            continue;
        };
        let receiver_type = receiver_text.as_deref().and_then(|receiver| {
            infer_receiver_type(receiver, caller, local_types, imports, package)
        });
        calls.push(RawJavaCall {
            path: caller.path.clone().unwrap_or_default(),
            file_hash: caller.file_hash.clone(),
            caller_symbol: caller.symbol_id.clone(),
            target_name,
            receiver_text,
            receiver_type,
            arg_count,
            range: point_range(node),
            dispatch_kind,
        });
    }
}

fn call_target(node: Node, source: &[u8]) -> Option<(String, Option<String>, usize, DispatchKind)> {
    match node.kind() {
        "method_invocation" => {
            let target = node
                .child_by_field_name("name")
                .and_then(|n| node_text(n, source))?;
            let receiver = node
                .child_by_field_name("object")
                .or_else(|| node.child_by_field_name("receiver"))
                .and_then(|n| node_text(n, source));
            let dispatch = receiver
                .as_deref()
                .map(|value| {
                    if value == "super" {
                        DispatchKind::Super
                    } else if value.chars().next().is_some_and(char::is_uppercase) {
                        DispatchKind::Static
                    } else {
                        DispatchKind::Virtual
                    }
                })
                .unwrap_or(DispatchKind::Virtual);
            Some((target, receiver, argument_count(node), dispatch))
        }
        "object_creation_expression" => {
            let target = node
                .child_by_field_name("type")
                .or_else(|| node.child_by_field_name("name"))
                .and_then(|n| node_text(n, source))?;
            Some((
                last_identifier(&erase_type(&target)),
                Some(erase_type(&target)),
                argument_count(node),
                DispatchKind::Constructor,
            ))
        }
        "super_method_invocation" => {
            let target = node
                .child_by_field_name("name")
                .and_then(|n| node_text(n, source))?;
            Some((
                target,
                Some("super".to_string()),
                argument_count(node),
                DispatchKind::Super,
            ))
        }
        "method_reference" => {
            let target = node
                .child_by_field_name("name")
                .and_then(|n| node_text(n, source))?;
            let receiver = node
                .child_by_field_name("type")
                .and_then(|n| node_text(n, source));
            Some((target, receiver, 0, DispatchKind::MethodReference))
        }
        _ => None,
    }
}

fn argument_count(node: Node) -> usize {
    let Some(arguments) = node.child_by_field_name("arguments") else {
        return 0;
    };
    named_children(arguments)
        .into_iter()
        .filter(|child| child.kind() != "," && child.kind() != "(" && child.kind() != ")")
        .count()
}

fn infer_receiver_type(
    receiver: &str,
    caller: &JavaSymbol,
    local_types: &BTreeMap<String, String>,
    imports: &[JavaImport],
    package: &str,
) -> Option<String> {
    match receiver {
        "this" => caller
            .qualified_name
            .split('#')
            .next()
            .map(ToString::to_string),
        "super" => None,
        value => local_types
            .get(value)
            .map(|type_name| qualify_type_name(type_name, imports, package))
            .or_else(|| {
                value
                    .chars()
                    .next()
                    .is_some_and(char::is_uppercase)
                    .then(|| qualify_type_name(value, imports, package))
            }),
    }
}

fn qualify_type_name(type_name: &str, imports: &[JavaImport], package: &str) -> String {
    let erased = erase_type(type_name);
    if erased.contains('.') {
        return erased;
    }
    if let Some(import) = imports.iter().find(|import| {
        !import.is_static && !import.is_wildcard && last_identifier(&import.path) == erased
    }) {
        return import.path.clone();
    }
    if package.is_empty() {
        erased
    } else {
        format!("{package}.{erased}")
    }
}

fn descendants(node: Node) -> impl Iterator<Item = Node> {
    let mut stack = vec![node];
    std::iter::from_fn(move || {
        let node = stack.pop()?;
        let mut children = named_children(node);
        children.reverse();
        stack.extend(children);
        Some(node)
    })
}

pub(crate) fn type_symbol_id(root_id: &str, qualified_name: &str) -> String {
    format!("java:{root_id}:type:{qualified_name}")
}

pub(crate) fn method_symbol_id(
    root_id: &str,
    owner_qualified_name: &str,
    name: &str,
    arity: usize,
) -> String {
    format!("java:{root_id}:method:{owner_qualified_name}#{name}/{arity}")
}

pub(crate) fn field_symbol_id(root_id: &str, owner_qualified_name: &str, name: &str) -> String {
    format!("java:{root_id}:field:{owner_qualified_name}#{name}")
}
