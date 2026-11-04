use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, fs::{self, File, OpenOptions}, io::{Read, Write}, path::{Path, PathBuf}, sync::Arc, time::{SystemTime, UNIX_EPOCH}};

pub type Result<T> = std::result::Result<T, String>;
pub type Chunks = Vec<Arc<String>>;
pub const CHUNK_CHARS: usize = 4000;
pub fn hash(bytes: &[u8]) -> String { format!("{:x}", Sha256::digest(bytes)) }
pub fn split(text: &str) -> Chunks {
    let mut result = Vec::new(); let mut start = 0; let mut count = 0;
    for (i, ch) in text.char_indices() {
        count += 1;
        if count >= CHUNK_CHARS || (ch == '\n' && count >= CHUNK_CHARS * 3 / 4) {
            let end = i + ch.len_utf8(); result.push(Arc::new(text[start..end].into())); start = end; count = 0;
        }
    }
    if start < text.len() || result.is_empty() { result.push(Arc::new(text[start..].into())); }
    result
}

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub enum Encoding { #[default] Utf8, Utf8Bom, Utf16Le, Utf16Be }

#[derive(Clone, Serialize, Deserialize)]
pub struct Snapshot { #[serde(default)] pub content_hash: String, pub chunks: Vec<String>, pub page: usize, pub encoding: Encoding }
#[derive(Clone, Serialize, Deserialize)]
pub struct Version { pub id: u64, pub time: u64, pub reason: String, pub snapshot: Snapshot }
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct History { pub versions: Vec<Version>, pub draft: Option<Snapshot>, pub next_id: u64 }

#[derive(Clone)]
pub struct Document {
    pub path: PathBuf, pub store: PathBuf, pub chunks: Chunks, pub page: usize,
    pub encoding: Encoding, pub revision: u64, pub drafted_revision: u64, pub dirty: bool,
    pub history: History, pub history_bytes: u64, pub version_skipped: bool,
    // Exclusive document lock is retained by all in-flight snapshots too.
    pub _lock: Arc<File>,
}

fn atomic(path: &Path, write: impl FnOnce(&mut File) -> Result<()>) -> Result<()> {
    let tmp = path.with_file_name(format!("{}.jingdu-tmp", path.file_name().unwrap_or_default().to_string_lossy()));
    let mut file = File::create(&tmp).map_err(|e| format!("無法建立暫存檔：{e}"))?;
    if let Err(e) = write(&mut file).and_then(|_| file.sync_all().map_err(|e| e.to_string())) { drop(file); let _ = fs::remove_file(&tmp); return Err(e); }
    drop(file);
    fs::rename(&tmp, path).map_err(|e| format!("無法保存 {}：{e}", path.display()))
}
fn save_history(store: &Path, history: &History) -> Result<()> {
    let bytes = serde_json::to_vec(history).map_err(|e| e.to_string())?;
    atomic(&store.join("history.json"), |f| f.write_all(&bytes).map_err(|e| e.to_string()))
}
fn object_path(store: &Path, id: &str) -> Result<PathBuf> {
    if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) { return Err("版本物件識別碼無效".into()); }
    Ok(store.join("objects").join(format!("{id}.gz")))
}
pub fn read_snapshot(store: &Path, snapshot: &Snapshot) -> Result<Chunks> {
    snapshot.chunks.iter().map(|id| {
        let file = File::open(object_path(store, id)?).map_err(|e| format!("版本資料遺失：{e}"))?;
        let mut bytes = Vec::new(); GzDecoder::new(file).take(128 * 1024 * 1024).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        if hash(&bytes) != *id { return Err("版本資料校驗失敗；未覆寫原檔".into()); }
        String::from_utf8(bytes).map(Arc::new).map_err(|e| e.to_string())
    }).collect()
}
fn snapshot(doc: &Document) -> Result<Snapshot> {
    let mut ids = Vec::with_capacity(doc.chunks.len());
    for chunk in &doc.chunks {
        let id = hash(chunk.as_bytes()); let path = object_path(&doc.store, &id)?;
        if !path.exists() {
            atomic(&path, |f| {
                let mut encoder = GzEncoder::new(f, Compression::fast());
                encoder.write_all(chunk.as_bytes()).map_err(|e| e.to_string())?;
                encoder.finish().map_err(|e| e.to_string())?; Ok(())
            })?;
        }
        ids.push(id);
    }
    let mut digest=Sha256::new();for chunk in &doc.chunks {digest.update(chunk.as_bytes());}
    Ok(Snapshot { content_hash: format!("{:x}",digest.finalize()), chunks: ids, page: doc.page, encoding: doc.encoding })
}
fn used(history: &History, draft: bool) -> BTreeSet<String> {
    let mut ids: BTreeSet<_> = history.versions.iter().flat_map(|v| v.snapshot.chunks.iter().cloned()).collect();
    if draft { if let Some(s) = &history.draft { ids.extend(s.chunks.iter().cloned()); } }
    ids
}
fn collect_unused(store: &Path, history: &History) -> Result<()> {
    let keep = used(history, true);
    for entry in fs::read_dir(store.join("objects")).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().is_some_and(|e| e == "gz") {
            let id = path.file_stem().unwrap().to_string_lossy();
            if !keep.contains(id.as_ref()) { fs::remove_file(path).map_err(|e| e.to_string())?; }
        }
    }
    Ok(())
}
fn history_bytes(store: &Path, history: &History) -> u64 {
    // Shared blobs are counted once. Draft-only blobs are excluded.
    used(history, false).iter().filter_map(|id| fs::metadata(object_path(store, id).ok()?).ok()).map(|m| m.len()).sum::<u64>()
        + serde_json::to_vec(&history.versions).map_or(0, |b| b.len() as u64)
}
pub fn open(root: &Path, path: PathBuf, create: bool) -> Result<Document> {
    let absolute = if path.is_absolute() { path.clone() } else { std::env::current_dir().map_err(|e|e.to_string())?.join(&path) };
    let canonical = fs::canonicalize(&absolute).unwrap_or(absolute);
    let identity = canonical.to_string_lossy().trim_start_matches("\\\\?\\").replace('/', "\\").to_lowercase();
    let store = root.join(hash(identity.as_bytes()));
    fs::create_dir_all(store.join("objects")).map_err(|e| e.to_string())?;
    let mut options = OpenOptions::new(); options.read(true).write(true).create(true);
    #[cfg(windows)] { use std::os::windows::fs::OpenOptionsExt; options.share_mode(0); }
    let lock = Arc::new(options.open(store.join("document.lock")).map_err(|e| format!("此文件可能已在另一個寫作視窗開啟：{e}"))?);
    let history = match fs::read(store.join("history.json")) {
        Ok(bytes) => serde_json::from_slice::<History>(&bytes).map_err(|e| format!("版本紀錄損壞：{e}"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => History::default(),
        Err(e) => return Err(e.to_string()),
    };
    let (chunks, encoding, page, dirty) = if let Some(draft) = &history.draft {
        (read_snapshot(&store, draft)?, draft.encoding, draft.page, true)
    } else {
        let bytes = if create && !path.exists() { Vec::new() } else { fs::read(&path).map_err(|e| e.to_string())? };
        let encoding = if bytes.starts_with(&[255,254]) { Encoding::Utf16Le } else if bytes.starts_with(&[254,255]) { Encoding::Utf16Be } else if bytes.starts_with(&[239,187,191]) { Encoding::Utf8Bom } else { Encoding::Utf8 };
        (split(&crate::decode(bytes)?), encoding, 0, false)
    };
    let page = page.min(chunks.len().saturating_sub(1));
    Ok(Document { path, version_skipped: false, history_bytes: history_bytes(&store, &history), store, chunks, page, encoding, revision: 0, drafted_revision: 0, dirty, history, _lock: lock })
}
fn write_original(doc: &Document) -> Result<()> {
    atomic(&doc.path, |file| {
        let bom: &[u8] = match doc.encoding { Encoding::Utf8 => &[], Encoding::Utf8Bom => &[239,187,191], Encoding::Utf16Le => &[255,254], Encoding::Utf16Be => &[254,255] };
        let mut writer = std::io::BufWriter::new(file);
        writer.write_all(bom).map_err(|e| e.to_string())?;
        for chunk in &doc.chunks {
            match doc.encoding {
                Encoding::Utf8 | Encoding::Utf8Bom => writer.write_all(chunk.as_bytes()).map_err(|e| e.to_string())?,
                Encoding::Utf16Le | Encoding::Utf16Be => {
                    for unit in chunk.encode_utf16() { writer.write_all(&if matches!(doc.encoding, Encoding::Utf16Le) { unit.to_le_bytes() } else { unit.to_be_bytes() }).map_err(|e| e.to_string())?; }
                }
            }
        }
        writer.flush().map_err(|e| e.to_string())
    })
}
pub fn save(mut doc: Document, reason: Option<&str>) -> Result<Document> {
    // Retry after a partial failure must retain any history already committed on disk.
    if let Ok(bytes) = fs::read(doc.store.join("history.json")) { doc.history = serde_json::from_slice(&bytes).map_err(|e|format!("版本紀錄無法讀取：{e}"))?; }
    doc.version_skipped=false;
    let snap = snapshot(&doc)?;
    // First persist a recovery draft, so a failure after replacing the TXT remains recoverable.
    doc.history.draft = Some(snap.clone()); save_history(&doc.store, &doc.history)?;
    if let Some(reason) = reason {
        write_original(&doc)?;
        let mut repeats=0;
        for version in doc.history.versions.iter().rev().take(2) {
            let previous=&version.snapshot;
            let identical=previous.encoding==snap.encoding && if previous.content_hash.is_empty() {
                if previous.chunks==snap.chunks {true} else {
                    let mut digest=Sha256::new();for chunk in read_snapshot(&doc.store,previous)? {digest.update(chunk.as_bytes());}
                    format!("{:x}",digest.finalize())==snap.content_hash
                }
            } else {previous.content_hash==snap.content_hash};
            if !identical {break;} repeats+=1;
        }
        doc.version_skipped=repeats>=2;
        if repeats<2 {
        doc.history.next_id += 1;
        doc.history.versions.push(Version { id: doc.history.next_id, time: SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(), reason: reason.into(), snapshot: snap });
        }
        doc.history.draft = None;
        save_history(&doc.store, &doc.history)?;
        doc.dirty = false;
    }
    doc.drafted_revision = doc.revision;
    collect_unused(&doc.store, &doc.history)?;
    doc.history_bytes = history_bytes(&doc.store, &doc.history);
    Ok(doc)
}
pub fn restore(doc: Document, id: u64) -> Result<Document> {
    let snap = doc.history.versions.iter().find(|v| v.id == id).ok_or("版本不存在")?.snapshot.clone();
    // Verify target before touching the source file.
    let chunks = read_snapshot(&doc.store, &snap)?;
    let mut doc = save(doc, Some("還原前備份"))?;
    doc.chunks = chunks; doc.page = snap.page.min(doc.chunks.len().saturating_sub(1)); doc.encoding = snap.encoding; doc.revision += 1;
    save(doc, Some(&format!("還原版本 #{id}")))
}
pub fn delete(mut doc: Document, id: u64) -> Result<Document> {
    doc.history.versions.retain(|v| v.id != id);
    save_history(&doc.store, &doc.history)?;
    collect_unused(&doc.store, &doc.history)?;
    doc.history_bytes = history_bytes(&doc.store, &doc.history);
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup(name: &str) -> (PathBuf, PathBuf) {
        let root = PathBuf::from("verification/writing").join(format!("{name}-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir_all(&root).unwrap(); let path = root.join("book.txt"); fs::write(&path,"原文🙂\r\n").unwrap(); (root.join("history"),path)
    }
    #[test]
    fn drafts_are_bounded_no_nodes_and_manual_identical_nodes_share_objects() {
        let (root,path) = setup("draft"); let mut doc = open(&root,path.clone(),false).unwrap();
        for n in 0..5 { doc.chunks = split(&format!("草稿{n}")); doc.revision += 1; doc = save(doc,None).unwrap(); }
        assert!(doc.history.versions.is_empty()); assert_eq!(fs::read_to_string(&path).unwrap(),"原文🙂\r\n");
        assert_eq!(fs::read_dir(doc.store.join("objects")).unwrap().count(),1);
        drop(doc); let doc = open(&root,path.clone(),false).unwrap(); assert_eq!(&**doc.chunks.first().unwrap(),"草稿4");
        let doc = save(doc,Some("儲存")).unwrap(); let doc = save(doc,Some("關閉")).unwrap();
        assert_eq!(doc.history.versions.len(),2); assert_eq!(doc.history.versions[0].snapshot.chunks,doc.history.versions[1].snapshot.chunks);
        assert_eq!(fs::read_dir(doc.store.join("objects")).unwrap().count(),1); assert!(doc.history.draft.is_none());
        assert_eq!(fs::read_to_string(path).unwrap(),"草稿4");
    }
    #[test]
    fn restore_backs_up_unsaved_work_and_deletion_keeps_shared_data() {
        let (root,path)=setup("restore"); let doc=open(&root,path.clone(),false).unwrap(); let mut doc=save(doc,Some("儲存")).unwrap();
        doc.chunks=split("未儲存修改"); doc=restore(doc,1).unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(),"原文🙂\r\n"); assert_eq!(doc.history.versions.len(),3);
        assert_eq!(&**read_snapshot(&doc.store,&doc.history.versions[1].snapshot).unwrap().first().unwrap(),"未儲存修改");
        doc=delete(doc,1).unwrap(); assert!(read_snapshot(&doc.store,&doc.history.versions[1].snapshot).is_ok());
    }
    #[test]
    fn utf16_round_trip_and_failed_save_preserves_recovery() {
        let (root,path)=setup("utf16"); let mut bytes=vec![255,254]; for u in "中文🙂\r\n".encode_utf16(){bytes.extend(u.to_le_bytes());} fs::write(&path,&bytes).unwrap();
        let doc=open(&root,path.clone(),false).unwrap(); let doc=save(doc,Some("儲存")).unwrap(); assert_eq!(fs::read(&path).unwrap(),bytes);
        let mut failed=doc.clone(); failed.path=path.join("invalid.txt"); assert!(save(failed,Some("失敗")).is_err());
        assert_eq!(fs::read(path).unwrap(),bytes); let history:History=serde_json::from_slice(&fs::read(doc.store.join("history.json")).unwrap()).unwrap(); assert!(history.draft.is_some());
    }
    #[test]
    fn new_file_keeps_same_store_after_first_save_and_reopen() {
        let (root,original)=setup("new"); let path=original.with_file_name("new.txt");
        let mut doc=open(&root,path.clone(),true).unwrap();doc.chunks=split("新作品");doc=save(doc,Some("初次儲存")).unwrap();
        let store=doc.store.clone();drop(doc);let doc=open(&root,path,false).unwrap();assert_eq!(doc.store,store);assert_eq!(doc.history.versions.len(),1);
    }
    #[test]
    fn large_document_repeated_saves_reuse_unchanged_chunks() {
        let (root,path)=setup("large");let paragraph="這是大型文件的測試段落，用來檢查分塊保存、壓縮與版本去重。\n".repeat(200);
        let text=paragraph.repeat(6000);assert!(text.len()>100_000_000);fs::write(&path,&text).unwrap();drop(text);
        let start=std::time::Instant::now();let doc=open(&root,path.clone(),false).unwrap();let mut doc=save(doc,Some("初版")).unwrap();
        let original_ids=doc.history.versions[0].snapshot.chunks.clone();let before=fs::read_dir(doc.store.join("objects")).unwrap().count();
        Arc::make_mut(&mut doc.chunks[1]).push_str("小修改");doc=save(doc,Some("修改")).unwrap();
        assert_eq!(&doc.history.versions[1].snapshot.chunks[2..],&original_ids[2..]);
        assert_eq!(fs::read_dir(doc.store.join("objects")).unwrap().count(),before+1);
        println!("Large writing open + two saves: {:.2}s; {} chunks, {} compressed bytes",start.elapsed().as_secs_f64(),doc.chunks.len(),doc.history_bytes);
    }
}

#[cfg(test)] mod duplicate_tests {
    use super::*;
    #[test] fn repeated_save_is_limited_even_after_reopen() {
        let root=PathBuf::from("verification").join(format!("duplicate-{}",SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir_all(&root).unwrap();let path=root.join("book.txt");fs::write(&path,"相同🙂").unwrap();
        let mut doc=open(&root,path.clone(),false).unwrap();
        for _ in 0..5 {doc=save(doc,Some("手動儲存")).unwrap();}
        assert_eq!(doc.history.versions.len(),2);
        let index=doc.store.join("history.json");
        let mut legacy:serde_json::Value=serde_json::from_slice(&fs::read(&index).unwrap()).unwrap();
        for version in legacy["versions"].as_array_mut().unwrap(){version["snapshot"].as_object_mut().unwrap().remove("content_hash");}
        fs::write(&index,serde_json::to_vec(&legacy).unwrap()).unwrap();
        drop(doc);let mut doc=open(&root,path,false).unwrap();doc=save(doc,Some("關閉儲存")).unwrap();assert_eq!(doc.history.versions.len(),2);
        doc.chunks=vec![Arc::new("相同".into()),Arc::new("🙂".into())];
        doc=save(doc,Some("重分塊")).unwrap();assert_eq!(doc.history.versions.len(),2);assert!(doc.version_skipped);
        doc.chunks=split("改動");doc=save(doc,Some("修改")).unwrap();assert_eq!(doc.history.versions.len(),3);
        doc=save(doc,Some("儲存")).unwrap();doc=save(doc,Some("儲存")).unwrap();assert_eq!(doc.history.versions.len(),4);
    }
}
