use std::{
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use ignore::WalkBuilder;
use zip::ZipArchive;

use crate::{
    java_semantic::model::{JavaSymbol, JavaSymbolKind, ResolveConfidence, SymbolOrigin},
    workspace::Workspace,
};

const ACC_PUBLIC: u16 = 0x0001;
const ACC_PRIVATE: u16 = 0x0002;
const ACC_PROTECTED: u16 = 0x0004;
const ACC_STATIC: u16 = 0x0008;

#[derive(Clone, Debug)]
struct ClassSummary {
    name: String,
    methods: Vec<MemberSummary>,
    fields: Vec<MemberSummary>,
}

#[derive(Clone, Debug)]
struct MemberSummary {
    name: String,
    descriptor: String,
    access: u16,
}

pub fn load_classpath_symbols(workspace: &Workspace, root_id: &str) -> Vec<JavaSymbol> {
    let mut symbols = Vec::new();
    for dir in class_output_dirs(&workspace.root) {
        if dir.exists() {
            symbols.extend(load_class_dir(workspace, root_id, &dir));
        }
    }
    for jar in jar_candidates(&workspace.root) {
        symbols.extend(load_jar(root_id, &jar).unwrap_or_default());
    }
    symbols
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

fn load_class_dir(workspace: &Workspace, root_id: &str, dir: &Path) -> Vec<JavaSymbol> {
    let mut symbols = Vec::new();
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
        symbols.extend(summary_to_symbols(root_id, Some(rel_path), summary));
    }
    symbols
}

fn load_jar(root_id: &str, path: &Path) -> Result<Vec<JavaSymbol>> {
    let file =
        fs::File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut archive =
        ZipArchive::new(file).with_context(|| format!("failed to read jar {}", path.display()))?;
    let mut symbols = Vec::new();
    for idx in 0..archive.len() {
        let mut entry = archive.by_index(idx)?;
        if !entry.name().ends_with(".class") {
            continue;
        }
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        if let Ok(summary) = parse_class(&bytes) {
            symbols.extend(summary_to_symbols(
                root_id,
                Some(format!("{}!{}", path.display(), entry.name())),
                summary,
            ));
        }
    }
    Ok(symbols)
}

fn summary_to_symbols(
    root_id: &str,
    path: Option<String>,
    summary: ClassSummary,
) -> Vec<JavaSymbol> {
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
            "java:{root_id}:method:{}#{}/{}",
            summary.name,
            method.name,
            parameters.len()
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

    symbols
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
    let _super_class = reader.u2()?;
    let name = class_name(&constant_pool, this_class).unwrap_or_default();
    let interfaces = reader.u2()?;
    for _ in 0..interfaces {
        let _ = reader.u2()?;
    }
    let fields = read_members(&mut reader, &constant_pool)?;
    let methods = read_members(&mut reader, &constant_pool)?;
    Ok(ClassSummary {
        name,
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
