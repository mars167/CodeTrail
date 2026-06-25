use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use ignore::WalkBuilder;
use quick_xml::{events::Event, Reader};
use zip::ZipArchive;

use crate::{
    java_semantic::model::{
        JavaSymbol, JavaSymbolKind, JavaTypeEdge, ResolveConfidence, SymbolOrigin,
    },
    workspace::Workspace,
};

const ACC_PUBLIC: u16 = 0x0001;
const ACC_PRIVATE: u16 = 0x0002;
const ACC_PROTECTED: u16 = 0x0004;
const ACC_STATIC: u16 = 0x0008;

#[derive(Clone, Debug)]
struct ClassSummary {
    name: String,
    super_name: Option<String>,
    interfaces: Vec<String>,
    methods: Vec<MemberSummary>,
    fields: Vec<MemberSummary>,
}

#[derive(Clone, Debug)]
struct MemberSummary {
    name: String,
    descriptor: String,
    access: u16,
}

#[derive(Clone, Debug, Default)]
pub struct ClasspathSymbols {
    pub symbols: Vec<JavaSymbol>,
    pub type_edges: Vec<JavaTypeEdge>,
}

pub fn load_classpath_symbols(workspace: &Workspace, root_id: &str) -> Vec<JavaSymbol> {
    load_classpath(workspace, root_id, &BTreeSet::new()).symbols
}

pub fn load_classpath(
    workspace: &Workspace,
    root_id: &str,
    type_hints: &BTreeSet<String>,
) -> ClasspathSymbols {
    let mut symbols = Vec::new();
    let mut type_edges = Vec::new();
    for dir in class_output_dirs(&workspace.root) {
        if dir.exists() {
            let contribution = load_class_dir(workspace, root_id, &dir);
            symbols.extend(contribution.symbols);
            type_edges.extend(contribution.type_edges);
        }
    }
    for archive in classpath_archives(&workspace.root, type_hints) {
        let contribution = load_archive(root_id, &archive, type_hints).unwrap_or_default();
        symbols.extend(contribution.symbols);
        type_edges.extend(contribution.type_edges);
    }
    ClasspathSymbols {
        symbols,
        type_edges,
    }
}

fn class_output_dirs(root: &Path) -> Vec<PathBuf> {
    [
        "target/classes",
        "target/test-classes",
        "build/classes/java/main",
        "build/classes/java/test",
        "out/production",
    ]
    .into_iter()
    .map(|rel| root.join(rel))
    .collect()
}

fn jar_candidates(root: &Path) -> Vec<PathBuf> {
    let mut jars = Vec::new();
    for rel in ["lib", "libs", "target/dependency", "build/libs"] {
        let dir = root.join(rel);
        if !dir.exists() {
            continue;
        }
        for entry in WalkBuilder::new(&dir).hidden(false).build().flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "jar") {
                jars.push(path.to_path_buf());
            }
        }
    }
    jars
}

#[derive(Clone, Debug, Default)]
struct MavenPom {
    properties: BTreeMap<String, String>,
    dependencies: Vec<MavenDependency>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Ord, PartialOrd)]
struct MavenDependency {
    group_id: String,
    artifact_id: String,
    version: Option<String>,
    classifier: Option<String>,
    packaging: Option<String>,
    scope: Option<String>,
}

fn maven_dependency_jars(root: &Path) -> Vec<PathBuf> {
    let Some(repo) = maven_repo() else {
        return Vec::new();
    };
    let poms = workspace_poms(root)
        .into_iter()
        .filter_map(|pom| parse_maven_pom(&pom).ok())
        .collect::<Vec<_>>();
    if poms.is_empty() {
        return Vec::new();
    }

    let mut properties = BTreeMap::new();
    for pom in &poms {
        properties.extend(pom.properties.clone());
    }
    let dependencies = poms
        .iter()
        .flat_map(|pom| pom.dependencies.iter().cloned())
        .collect::<BTreeSet<_>>();
    let mut managed_versions = BTreeMap::<(String, String, Option<String>), String>::new();
    for dep in &dependencies {
        let Some(version) = resolve_optional_value(dep.version.as_deref(), &properties) else {
            continue;
        };
        managed_versions.insert(
            (
                dep.group_id.clone(),
                dep.artifact_id.clone(),
                dep.classifier.clone(),
            ),
            version,
        );
    }

    let mut jars = Vec::new();
    let mut seen = BTreeSet::new();
    for dep in dependencies {
        if dep.group_id.is_empty()
            || dep.artifact_id.is_empty()
            || dep.scope.as_deref() == Some("test")
            || dep.packaging.as_deref() == Some("pom")
        {
            continue;
        }
        let version = resolve_optional_value(dep.version.as_deref(), &properties).or_else(|| {
            managed_versions
                .get(&(
                    dep.group_id.clone(),
                    dep.artifact_id.clone(),
                    dep.classifier.clone(),
                ))
                .cloned()
        });
        let jar = if let Some(version) = version {
            maven_jar_for_version(
                &repo,
                &dep.group_id,
                &dep.artifact_id,
                &version,
                dep.classifier.as_deref(),
            )
        } else {
            latest_maven_jar(
                &repo,
                &dep.group_id,
                &dep.artifact_id,
                dep.classifier.as_deref(),
            )
        };
        if let Some(jar) = jar {
            let key = jar.to_string_lossy().to_string();
            if seen.insert(key) {
                jars.push(jar);
            }
        }
    }
    jars
}

fn maven_repo() -> Option<PathBuf> {
    env::var_os("MAVEN_REPO_LOCAL")
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".m2/repository"))
        })
        .filter(|path| path.exists())
}

fn workspace_poms(root: &Path) -> Vec<PathBuf> {
    WalkBuilder::new(root)
        .hidden(false)
        .build()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.file_name().is_some_and(|name| name == "pom.xml")
                && !path_components_contain(path, &["target", ".codetrail"])
            {
                Some(path.to_path_buf())
            } else {
                None
            }
        })
        .collect()
}

fn path_components_contain(path: &Path, blocked: &[&str]) -> bool {
    path.components().any(|component| {
        let value = component.as_os_str().to_string_lossy();
        blocked.iter().any(|blocked| value == *blocked)
    })
}

fn parse_maven_pom(path: &Path) -> Result<MavenPom> {
    let source = fs::read_to_string(path)
        .with_context(|| format!("failed to read Maven pom {}", path.display()))?;
    let mut reader = Reader::from_str(&source);
    reader.config_mut().trim_text(true);
    reader.config_mut().check_end_names = true;
    let mut buf = Vec::new();
    let mut stack = Vec::<String>::new();
    let mut pom = MavenPom::default();
    let mut current_dependency: Option<MavenDependency> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(element)) => {
                let tag = String::from_utf8_lossy(element.local_name().as_ref()).to_string();
                if tag == "dependency" && !stack.iter().any(|item| item == "exclusion") {
                    current_dependency = Some(MavenDependency::default());
                }
                stack.push(tag);
            }
            Ok(Event::End(element)) => {
                let tag = String::from_utf8_lossy(element.local_name().as_ref()).to_string();
                if tag == "dependency" {
                    if let Some(dep) = current_dependency.take() {
                        if !dep.group_id.is_empty() && !dep.artifact_id.is_empty() {
                            pom.dependencies.push(dep);
                        }
                    }
                }
                stack.pop();
            }
            Ok(Event::Text(text)) => {
                let value = text.decode()?.trim().to_string();
                if value.is_empty() {
                    buf.clear();
                    continue;
                }
                if let Some(tag) = stack.last() {
                    if stack.iter().any(|item| item == "properties") && tag != "properties" {
                        pom.properties.insert(tag.clone(), value.clone());
                    }
                    if let Some(dep) = current_dependency.as_mut() {
                        if !stack.iter().any(|item| item == "exclusion") {
                            match tag.as_str() {
                                "groupId" => dep.group_id = value,
                                "artifactId" => dep.artifact_id = value,
                                "version" => dep.version = Some(value),
                                "classifier" => dep.classifier = Some(value),
                                "type" => dep.packaging = Some(value),
                                "scope" => dep.scope = Some(value),
                                _ => {}
                            }
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(error) => {
                anyhow::bail!(
                    "failed to parse Maven pom {} at byte {}: {error}",
                    path.display(),
                    reader.error_position()
                );
            }
        }
        buf.clear();
    }
    Ok(pom)
}

fn resolve_optional_value(
    value: Option<&str>,
    properties: &BTreeMap<String, String>,
) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    let mut resolved = value.to_string();
    for _ in 0..8 {
        let Some(start) = resolved.find("${") else {
            break;
        };
        let Some(end_offset) = resolved[start + 2..].find('}') else {
            break;
        };
        let end = start + 2 + end_offset;
        let key = &resolved[start + 2..end];
        let Some(replacement) = properties.get(key) else {
            return None;
        };
        resolved.replace_range(start..=end, replacement);
    }
    (!resolved.contains("${")).then_some(resolved)
}

fn maven_jar_for_version(
    repo: &Path,
    group_id: &str,
    artifact_id: &str,
    version: &str,
    classifier: Option<&str>,
) -> Option<PathBuf> {
    let dir = repo
        .join(group_id.replace('.', "/"))
        .join(artifact_id)
        .join(version);
    let suffix = classifier
        .filter(|value| !value.is_empty())
        .map(|value| format!("-{value}"))
        .unwrap_or_default();
    let jar = dir.join(format!("{artifact_id}-{version}{suffix}.jar"));
    jar.exists().then_some(jar)
}

fn latest_maven_jar(
    repo: &Path,
    group_id: &str,
    artifact_id: &str,
    classifier: Option<&str>,
) -> Option<PathBuf> {
    let dir = repo.join(group_id.replace('.', "/")).join(artifact_id);
    let versions = fs::read_dir(dir).ok()?;
    versions
        .flatten()
        .filter_map(|entry| {
            let version = entry.file_name().to_string_lossy().to_string();
            maven_jar_for_version(repo, group_id, artifact_id, &version, classifier)
                .map(|jar| (version, jar))
        })
        .max_by(|(a, _), (b, _)| compare_versions(a, b))
        .map(|(_, jar)| jar)
}

fn compare_versions(a: &str, b: &str) -> Ordering {
    let left = version_tokens(a);
    let right = version_tokens(b);
    for idx in 0..left.len().max(right.len()) {
        let a = left.get(idx).copied().unwrap_or(0);
        let b = right.get(idx).copied().unwrap_or(0);
        match a.cmp(&b) {
            Ordering::Equal => {}
            other => return other,
        }
    }
    a.cmp(b)
}

fn version_tokens(value: &str) -> Vec<u64> {
    value
        .split(|ch: char| !ch.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse::<u64>().ok())
        .collect()
}

fn jdk_base_archives() -> Vec<PathBuf> {
    let mut archives = Vec::new();
    if let Some(java_home) = env::var_os("JAVA_HOME").map(PathBuf::from) {
        for rel in [
            "lib/jmods/java.base.jmod",
            "jmods/java.base.jmod",
            "jre/lib/rt.jar",
        ] {
            let path = java_home.join(rel);
            if path.exists() {
                archives.push(path);
            }
        }
    }
    archives
}

fn classpath_archives(root: &Path, type_hints: &BTreeSet<String>) -> Vec<PathBuf> {
    let mut seen = BTreeSet::new();
    let mut archives = Vec::new();
    for path in jar_candidates(root)
        .into_iter()
        .chain(maven_dependency_jars(root))
        .chain(maven_import_hint_jars(type_hints))
        .chain(jdk_base_archives())
    {
        let normalized = path.to_string_lossy().to_string();
        if seen.insert(normalized) {
            archives.push(path);
        }
    }
    archives
}

fn maven_import_hint_jars(type_hints: &BTreeSet<String>) -> Vec<PathBuf> {
    let Some(repo) = maven_repo() else {
        return Vec::new();
    };
    let mut jars = Vec::new();
    let mut seen = BTreeSet::new();
    for hint in type_hints {
        if hint.starts_with("java.") || hint.starts_with("javax.") || hint.starts_with("jakarta.") {
            continue;
        }
        let parts = hint.split('.').collect::<Vec<_>>();
        if parts.len() < 3 {
            continue;
        }
        let package_len = parts.len() - 1;
        let prefix_len = package_len.min(3);
        let dir = repo.join(parts[..prefix_len].join("/"));
        if !dir.exists() {
            continue;
        }
        for entry in WalkBuilder::new(&dir)
            .hidden(false)
            .max_depth(Some(6))
            .build()
            .flatten()
        {
            let path = entry.path();
            if !path.extension().is_some_and(|ext| ext == "jar") {
                continue;
            }
            let key = path.to_string_lossy().to_string();
            if seen.insert(key) {
                jars.push(path.to_path_buf());
            }
        }
    }
    jars
}

fn load_class_dir(workspace: &Workspace, root_id: &str, dir: &Path) -> ClasspathSymbols {
    let mut symbols = Vec::new();
    let mut type_edges = Vec::new();
    for entry in WalkBuilder::new(dir).hidden(false).build().flatten() {
        let path = entry.path();
        if !path.extension().is_some_and(|ext| ext == "class") {
            continue;
        }
        let Ok(bytes) = fs::read(path) else {
            continue;
        };
        let Ok(summary) = parse_class(&bytes) else {
            continue;
        };
        let rel_path = path
            .strip_prefix(&workspace.root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let contribution = summary_to_symbols(root_id, Some(rel_path), summary);
        symbols.extend(contribution.symbols);
        type_edges.extend(contribution.type_edges);
    }
    ClasspathSymbols {
        symbols,
        type_edges,
    }
}

fn load_archive(
    root_id: &str,
    path: &Path,
    type_hints: &BTreeSet<String>,
) -> Result<ClasspathSymbols> {
    let file =
        fs::File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut archive =
        ZipArchive::new(file).with_context(|| format!("failed to read {}", path.display()))?;
    if !type_hints.is_empty() {
        return load_targeted_archive(root_id, path, &mut archive, type_hints);
    }
    let mut symbols = Vec::new();
    let mut type_edges = Vec::new();
    for idx in 0..archive.len() {
        let mut entry = archive.by_index(idx)?;
        if !entry.name().ends_with(".class") {
            continue;
        }
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        if let Ok(summary) = parse_class(&bytes) {
            let contribution = summary_to_symbols(
                root_id,
                Some(format!("{}!{}", path.display(), entry.name())),
                summary,
            );
            symbols.extend(contribution.symbols);
            type_edges.extend(contribution.type_edges);
        }
    }
    Ok(ClasspathSymbols {
        symbols,
        type_edges,
    })
}

fn load_targeted_archive<R: Read + std::io::Seek>(
    root_id: &str,
    path: &Path,
    archive: &mut ZipArchive<R>,
    type_hints: &BTreeSet<String>,
) -> Result<ClasspathSymbols> {
    let mut entries = BTreeMap::<String, usize>::new();
    for idx in 0..archive.len() {
        let name = archive.by_index(idx)?.name().to_string();
        if let Some(class_name) = archive_entry_class_name(&name) {
            entries.insert(class_name, idx);
        }
    }

    let mut symbols = Vec::new();
    let mut type_edges = Vec::new();
    let mut queue = type_hints.iter().cloned().collect::<Vec<_>>();
    let mut seen = BTreeSet::new();
    while let Some(class_name) = queue.pop() {
        if !seen.insert(class_name.clone()) {
            continue;
        }
        let Some(entry_idx) = entries.get(&class_name).copied() else {
            continue;
        };
        let mut entry = archive.by_index(entry_idx)?;
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        let Ok(summary) = parse_class(&bytes) else {
            continue;
        };
        if let Some(super_name) = &summary.super_name {
            queue.push(super_name.clone());
        }
        queue.extend(summary.interfaces.iter().cloned());
        let contribution = summary_to_symbols(
            root_id,
            Some(format!("{}!{}", path.display(), entry.name())),
            summary,
        );
        symbols.extend(contribution.symbols);
        type_edges.extend(contribution.type_edges);
    }
    Ok(ClasspathSymbols {
        symbols,
        type_edges,
    })
}

fn archive_entry_class_name(name: &str) -> Option<String> {
    let name = name
        .strip_prefix("classes/")
        .unwrap_or(name)
        .strip_suffix(".class")?;
    if name == "module-info" || name.ends_with("/package-info") {
        return None;
    }
    Some(name.replace('/', "."))
}

fn summary_to_symbols(
    root_id: &str,
    path: Option<String>,
    summary: ClassSummary,
) -> ClasspathSymbols {
    let package = summary
        .name
        .rsplit_once('.')
        .map(|(pkg, _)| pkg.to_string())
        .unwrap_or_default();
    let simple_name = summary
        .name
        .rsplit('.')
        .next()
        .unwrap_or(&summary.name)
        .to_string();
    let type_id = format!("java:{root_id}:type:{}", summary.name);
    let mut type_edges = Vec::new();
    if let Some(super_name) = &summary.super_name {
        if super_name != "java.lang.Object" {
            type_edges.push(JavaTypeEdge {
                subtype: type_id.clone(),
                supertype: super_name.clone(),
                relation: "extends".to_string(),
            });
        }
    }
    for interface in &summary.interfaces {
        type_edges.push(JavaTypeEdge {
            subtype: type_id.clone(),
            supertype: interface.clone(),
            relation: "implements".to_string(),
        });
    }
    let mut symbols = vec![JavaSymbol {
        symbol_id: type_id.clone(),
        name: simple_name.clone(),
        kind: JavaSymbolKind::Type,
        package: package.clone(),
        qualified_name: summary.name.clone(),
        owner_symbol: None,
        path: path.clone(),
        range: None,
        selection_range: None,
        descriptor: None,
        parameters: Vec::new(),
        return_type: None,
        modifiers: vec!["classfile_summary".to_string()],
        origin: SymbolOrigin::Classfile,
        confidence: ResolveConfidence::ClassfileSummary,
        root_id: root_id.to_string(),
        file_hash: String::new(),
    }];

    for method in summary.methods {
        if !public_or_protected(method.access) || method.name == "<clinit>" {
            continue;
        }
        let (parameters, return_type) = parse_method_descriptor(&method.descriptor);
        let public_name = if method.name == "<init>" {
            simple_name.clone()
        } else {
            method.name.clone()
        };
        let symbol_id = format!(
            "java:{root_id}:method:{}#{}({})",
            summary.name,
            method.name,
            parameters.join(",")
        );
        symbols.push(JavaSymbol {
            symbol_id,
            name: public_name,
            kind: if method.name == "<init>" {
                JavaSymbolKind::Constructor
            } else {
                JavaSymbolKind::Method
            },
            package: package.clone(),
            qualified_name: format!("{}#{}", summary.name, method.name),
            owner_symbol: Some(type_id.clone()),
            path: path.clone(),
            range: None,
            selection_range: None,
            descriptor: Some(method.descriptor),
            parameters,
            return_type,
            modifiers: member_modifiers(method.access),
            origin: SymbolOrigin::Classfile,
            confidence: ResolveConfidence::ClassfileSummary,
            root_id: root_id.to_string(),
            file_hash: String::new(),
        });
    }

    for field in summary.fields {
        if !public_or_protected(field.access) {
            continue;
        }
        let symbol_id = format!("java:{root_id}:field:{}#{}", summary.name, field.name);
        symbols.push(JavaSymbol {
            symbol_id,
            name: field.name.clone(),
            kind: JavaSymbolKind::Field,
            package: package.clone(),
            qualified_name: format!("{}#{}", summary.name, field.name),
            owner_symbol: Some(type_id.clone()),
            path: path.clone(),
            range: None,
            selection_range: None,
            descriptor: Some(field.descriptor.clone()),
            parameters: Vec::new(),
            return_type: Some(parse_field_descriptor(&field.descriptor)),
            modifiers: member_modifiers(field.access),
            origin: SymbolOrigin::Classfile,
            confidence: ResolveConfidence::ClassfileSummary,
            root_id: root_id.to_string(),
            file_hash: String::new(),
        });
    }

    ClasspathSymbols {
        symbols,
        type_edges,
    }
}

fn public_or_protected(access: u16) -> bool {
    access & ACC_PUBLIC != 0 || access & ACC_PROTECTED != 0
}

fn member_modifiers(access: u16) -> Vec<String> {
    let mut modifiers = vec!["classfile_summary".to_string()];
    if access & ACC_PUBLIC != 0 {
        modifiers.push("public".to_string());
    }
    if access & ACC_PROTECTED != 0 {
        modifiers.push("protected".to_string());
    }
    if access & ACC_PRIVATE != 0 {
        modifiers.push("private".to_string());
    }
    if access & ACC_STATIC != 0 {
        modifiers.push("static".to_string());
    }
    modifiers
}

#[derive(Clone, Debug)]
enum CpEntry {
    Utf8(String),
    Class(u16),
    Other,
}

fn parse_class(bytes: &[u8]) -> Result<ClassSummary> {
    let mut reader = ClassReader::new(bytes);
    if reader.u4()? != 0xCAFEBABE {
        anyhow::bail!("not a class file");
    }
    let _minor = reader.u2()?;
    let _major = reader.u2()?;
    let constant_pool = read_constant_pool(&mut reader)?;
    let _access_flags = reader.u2()?;
    let this_class = reader.u2()?;
    let super_class = reader.u2()?;
    let name = class_name(&constant_pool, this_class).unwrap_or_default();
    let super_name = (super_class != 0)
        .then(|| class_name(&constant_pool, super_class))
        .flatten();
    let interface_count = reader.u2()?;
    let mut interfaces = Vec::new();
    for _ in 0..interface_count {
        if let Some(interface) = class_name(&constant_pool, reader.u2()?) {
            interfaces.push(interface);
        }
    }
    let fields = read_members(&mut reader, &constant_pool)?;
    let methods = read_members(&mut reader, &constant_pool)?;
    Ok(ClassSummary {
        name,
        super_name,
        interfaces,
        methods,
        fields,
    })
}

fn read_constant_pool(reader: &mut ClassReader<'_>) -> Result<Vec<CpEntry>> {
    let count = reader.u2()? as usize;
    let mut pool = vec![CpEntry::Other; count];
    let mut idx = 1;
    while idx < count {
        let tag = reader.u1()?;
        pool[idx] = match tag {
            1 => {
                let len = reader.u2()? as usize;
                let bytes = reader.bytes(len)?;
                CpEntry::Utf8(String::from_utf8_lossy(bytes).to_string())
            }
            7 => CpEntry::Class(reader.u2()?),
            3 | 4 => {
                let _ = reader.u4()?;
                CpEntry::Other
            }
            5 | 6 => {
                let _ = reader.u4()?;
                let _ = reader.u4()?;
                idx += 1;
                CpEntry::Other
            }
            8 | 16 | 19 | 20 => {
                let _ = reader.u2()?;
                CpEntry::Other
            }
            9 | 10 | 11 | 12 | 17 | 18 => {
                let _ = reader.u2()?;
                let _ = reader.u2()?;
                CpEntry::Other
            }
            15 => {
                let _ = reader.u1()?;
                let _ = reader.u2()?;
                CpEntry::Other
            }
            _ => anyhow::bail!("unsupported constant pool tag {tag}"),
        };
        idx += 1;
    }
    Ok(pool)
}

fn read_members(reader: &mut ClassReader<'_>, pool: &[CpEntry]) -> Result<Vec<MemberSummary>> {
    let count = reader.u2()? as usize;
    let mut members = Vec::new();
    for _ in 0..count {
        let access = reader.u2()?;
        let name = utf8(pool, reader.u2()?).unwrap_or_default();
        let descriptor = utf8(pool, reader.u2()?).unwrap_or_default();
        let attributes = reader.u2()? as usize;
        for _ in 0..attributes {
            let _attribute_name = reader.u2()?;
            let len = reader.u4()? as usize;
            let _ = reader.bytes(len)?;
        }
        members.push(MemberSummary {
            name,
            descriptor,
            access,
        });
    }
    Ok(members)
}

fn class_name(pool: &[CpEntry], idx: u16) -> Option<String> {
    match pool.get(idx as usize)? {
        CpEntry::Class(name_idx) => utf8(pool, *name_idx).map(|name| name.replace('/', ".")),
        _ => None,
    }
}

fn utf8(pool: &[CpEntry], idx: u16) -> Option<String> {
    match pool.get(idx as usize)? {
        CpEntry::Utf8(value) => Some(value.clone()),
        _ => None,
    }
}

fn parse_method_descriptor(descriptor: &str) -> (Vec<String>, Option<String>) {
    let mut cursor = Cursor::new(descriptor.as_bytes());
    let mut params = Vec::new();
    let mut marker = [0u8; 1];
    if cursor.read_exact(&mut marker).is_err() || marker[0] != b'(' {
        return (params, None);
    }
    while cursor.read_exact(&mut marker).is_ok() && marker[0] != b')' {
        cursor.set_position(cursor.position().saturating_sub(1));
        params.push(parse_descriptor_type(&mut cursor));
    }
    let return_type = Some(parse_descriptor_type(&mut cursor));
    (params, return_type)
}

fn parse_field_descriptor(descriptor: &str) -> String {
    let mut cursor = Cursor::new(descriptor.as_bytes());
    parse_descriptor_type(&mut cursor)
}

fn parse_descriptor_type(cursor: &mut Cursor<&[u8]>) -> String {
    let mut marker = [0u8; 1];
    if cursor.read_exact(&mut marker).is_err() {
        return "Object".to_string();
    }
    match marker[0] {
        b'B' => "byte".to_string(),
        b'C' => "char".to_string(),
        b'D' => "double".to_string(),
        b'F' => "float".to_string(),
        b'I' => "int".to_string(),
        b'J' => "long".to_string(),
        b'S' => "short".to_string(),
        b'Z' => "boolean".to_string(),
        b'V' => "void".to_string(),
        b'[' => format!("{}[]", parse_descriptor_type(cursor)),
        b'L' => {
            let mut bytes = Vec::new();
            while cursor.read_exact(&mut marker).is_ok() && marker[0] != b';' {
                bytes.push(marker[0]);
            }
            String::from_utf8_lossy(&bytes).replace('/', ".")
        }
        _ => "Object".to_string(),
    }
}

struct ClassReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> ClassReader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn u1(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }

    fn u2(&mut self) -> Result<u16> {
        let bytes = self.bytes(2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn u4(&mut self) -> Result<u32> {
        let bytes = self.bytes(4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn bytes(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self.offset + len;
        if end > self.bytes.len() {
            anyhow::bail!("unexpected end of class file");
        }
        let slice = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(slice)
    }
}
