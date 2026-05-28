use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

use anyhow::{anyhow, Context, Result};

use crate::workspace::{FileRecord, Workspace};

const DOCS_MAGIC: &[u8; 8] = b"CSDOCS1\0";
const PATHS_MAGIC: &[u8; 8] = b"CSPATH1\0";
const GRAMS_MAGIC: &[u8; 8] = b"CSGRAM1\0";

pub fn write(
    text_root: &Path,
    workspace: &Workspace,
    records: &[FileRecord],
    include_grams: bool,
) -> Result<()> {
    fs::create_dir_all(text_root)?;
    write_docs(&text_root.join("docs.idx"), records)?;
    write_paths(&text_root.join("paths.idx"), records)?;
    if include_grams {
        write_grams(&text_root.join("grams.idx"), workspace, records)?;
    } else {
        write_empty_grams(&text_root.join("grams.idx"))?;
    }
    Ok(())
}

pub fn read_docs(path: &Path) -> Result<Vec<FileRecord>> {
    let mut file =
        File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    read_magic(&mut file, DOCS_MAGIC)?;
    let count = read_u32(&mut file)? as usize;
    let mut records = Vec::with_capacity(count);
    for _ in 0..count {
        let path = read_string(&mut file)?;
        let language = read_string(&mut file)?;
        let size = read_u64(&mut file)?;
        let mtime_ms = read_u128(&mut file)?;
        let hash = read_string(&mut file)?;
        records.push(FileRecord {
            path,
            language,
            size,
            mtime_ms,
            hash,
        });
    }
    Ok(records)
}

pub fn candidate_ids(path: &Path, pattern: &str, mode: &str) -> Result<Option<HashSet<usize>>> {
    if mode != "literal" || pattern.as_bytes().len() < 3 {
        return Ok(None);
    }
    let query_grams = grams_for_bytes(pattern.as_bytes());
    if query_grams.is_empty() {
        return Ok(None);
    }

    let postings = read_selected_grams(path, &query_grams)?;
    let mut candidate: Option<HashSet<usize>> = None;
    for gram in query_grams {
        let Some(ids) = postings.get(&gram) else {
            return Ok(Some(HashSet::new()));
        };
        let current = ids.iter().copied().collect::<HashSet<_>>();
        candidate = Some(match candidate {
            Some(existing) => existing.intersection(&current).copied().collect(),
            None => current,
        });
    }
    Ok(candidate)
}

fn write_docs(path: &Path, records: &[FileRecord]) -> Result<()> {
    let mut file = File::create(path)?;
    file.write_all(DOCS_MAGIC)?;
    write_u32(&mut file, records.len() as u32)?;
    for record in records {
        write_string(&mut file, &record.path)?;
        write_string(&mut file, &record.language)?;
        write_u64(&mut file, record.size)?;
        write_u128(&mut file, record.mtime_ms)?;
        write_string(&mut file, &record.hash)?;
    }
    Ok(())
}

fn write_paths(path: &Path, records: &[FileRecord]) -> Result<()> {
    let mut file = File::create(path)?;
    file.write_all(PATHS_MAGIC)?;
    write_u32(&mut file, records.len() as u32)?;
    for record in records {
        write_string(&mut file, &record.path)?;
    }
    Ok(())
}

fn write_grams(path: &Path, workspace: &Workspace, records: &[FileRecord]) -> Result<()> {
    let mut index = BTreeMap::<[u8; 3], Vec<u32>>::new();
    for (doc_id, record) in records.iter().enumerate() {
        let bytes = match fs::read(workspace.abs_path(&record.path)) {
            Ok(bytes) => bytes,
            Err(_) => continue,
        };
        for gram in grams_for_bytes(&bytes) {
            index.entry(gram).or_default().push(doc_id as u32);
        }
    }

    let mut file = File::create(path)?;
    file.write_all(GRAMS_MAGIC)?;
    write_u32(&mut file, index.len() as u32)?;
    for (gram, mut ids) in index {
        ids.sort_unstable();
        ids.dedup();
        file.write_all(&gram)?;
        write_u32(&mut file, ids.len() as u32)?;
        for id in ids {
            write_u32(&mut file, id)?;
        }
    }
    Ok(())
}

fn write_empty_grams(path: &Path) -> Result<()> {
    let mut file = File::create(path)?;
    file.write_all(GRAMS_MAGIC)?;
    write_u32(&mut file, 0)?;
    Ok(())
}

fn read_selected_grams(
    path: &Path,
    wanted: &HashSet<[u8; 3]>,
) -> Result<BTreeMap<[u8; 3], Vec<usize>>> {
    let mut file =
        File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    read_magic(&mut file, GRAMS_MAGIC)?;
    let count = read_u32(&mut file)? as usize;
    let mut postings = BTreeMap::new();
    for _ in 0..count {
        let mut gram = [0u8; 3];
        file.read_exact(&mut gram)?;
        let ids_len = read_u32(&mut file)? as usize;
        if wanted.contains(&gram) {
            let mut ids = Vec::with_capacity(ids_len);
            for _ in 0..ids_len {
                ids.push(read_u32(&mut file)? as usize);
            }
            postings.insert(gram, ids);
        } else {
            file.seek(SeekFrom::Current((ids_len * 4) as i64))?;
        }
    }
    Ok(postings)
}

fn grams_for_bytes(bytes: &[u8]) -> HashSet<[u8; 3]> {
    bytes
        .windows(3)
        .map(|window| [window[0], window[1], window[2]])
        .collect()
}

fn read_magic(file: &mut File, expected: &[u8; 8]) -> Result<()> {
    let mut actual = [0u8; 8];
    file.read_exact(&mut actual)?;
    if &actual != expected {
        return Err(anyhow!("invalid index magic"));
    }
    Ok(())
}

fn write_string(file: &mut File, value: &str) -> Result<()> {
    let bytes = value.as_bytes();
    write_u32(file, bytes.len() as u32)?;
    file.write_all(bytes)?;
    Ok(())
}

fn read_string(file: &mut File) -> Result<String> {
    let len = read_u32(file)? as usize;
    let mut bytes = vec![0u8; len];
    file.read_exact(&mut bytes)?;
    Ok(String::from_utf8(bytes)?)
}

fn write_u32(file: &mut File, value: u32) -> Result<()> {
    file.write_all(&value.to_le_bytes())?;
    Ok(())
}

fn read_u32(file: &mut File) -> Result<u32> {
    let mut bytes = [0u8; 4];
    file.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn write_u64(file: &mut File, value: u64) -> Result<()> {
    file.write_all(&value.to_le_bytes())?;
    Ok(())
}

fn read_u64(file: &mut File) -> Result<u64> {
    let mut bytes = [0u8; 8];
    file.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

fn write_u128(file: &mut File, value: u128) -> Result<()> {
    file.write_all(&value.to_le_bytes())?;
    Ok(())
}

fn read_u128(file: &mut File) -> Result<u128> {
    let mut bytes = [0u8; 16];
    file.read_exact(&mut bytes)?;
    Ok(u128::from_le_bytes(bytes))
}
