//! Cross-archive resource lookup for the city importer (RE/08 §4, RE/17 §2).
//!
//! City entities reference meshes, materials, textures and collision stored in other files of the same archive and in
//! `DataPC_Common.forge` / `DataPC.forge` (RE/15 §3, "cross-forge links"). The index maps every resource id to the
//! stored file that holds it, from the files' tables of contents only. Decompressed files are kept in a byte-budgeted
//! LRU so shared files (texture packs, prop meshes) are inflated once while the streamer walks the city. Reads are
//! positional, so loader threads share the open archives. Read-only: nothing from the install is written or copied.

use std::{collections::HashMap, path::Path, sync::{Arc, Mutex}};

use crate::assets::forge::{Forge, Resource};

/// PORT: memory budget for inflated stored files; the game's own resource manager budget is not reversed.
pub const FILE_CACHE_BYTES: usize = 384 << 20;

pub struct Archives {
    pub forges: Vec<Forge>,
    pub names: Vec<String>,
    /// resource id → (archive, stored file index)
    index: HashMap<u32, (u16, u32)>,
    cache: Mutex<FileCache>,
}

type FileKey = (u16, u32);

#[derive(Default)]
struct FileCache {
    files: HashMap<FileKey, (Arc<HashMap<u32, Arc<Resource>>>, usize, u64)>,
    bytes: usize,
    clock: u64,
}

impl Archives {
    /// Open `archives` in priority order (the first archive holding an id wins) and index their resource ids.
    pub fn open(game: &Path, archives: &[&str]) -> Result<Self, String> {
        let mut forges = Vec::new();
        let mut index = HashMap::new();
        for (a, name) in archives.iter().enumerate() {
            let forge = Forge::open(&game.join(name)).map_err(|e| format!("{name}: {e}"))?;
            for entry in &forge.entries {
                for id in forge.resource_ids_shared(entry).map_err(|e| format!("{name}/{}: {e}", entry.name))? {
                    index.entry(id).or_insert((a as u16, entry.index as u32));
                }
            }
            forges.push(forge);
        }
        Ok(Self { forges, names: archives.iter().map(|s| s.to_string()).collect(), index, cache: Mutex::default() })
    }

    pub fn contains(&self, id: u32) -> bool {
        self.index.contains_key(&id)
    }

    /// The stored file holding `id`, as (archive, file name).
    pub fn location(&self, id: u32) -> Option<(&str, &str)> {
        let &(a, f) = self.index.get(&id)?;
        Some((&self.names[a as usize], &self.forges[a as usize].entries[f as usize].name))
    }

    /// All resources of one stored file, inflated once and cached.
    pub fn file(&self, archive: usize, entry: usize) -> Result<Arc<HashMap<u32, Arc<Resource>>>, String> {
        let key = (archive as u16, entry as u32);
        {
            let mut cache = self.cache.lock().unwrap();
            cache.clock += 1;
            let clock = cache.clock;
            if let Some(hit) = cache.files.get_mut(&key) {
                hit.2 = clock;
                return Ok(hit.0.clone());
            }
        }
        // Inflate outside the lock: several loader threads decompress different files at once.
        let forge = &self.forges[archive];
        let stored = &forge.entries[entry];
        let resources = forge.resources_shared(stored).map_err(|e| format!("{}: {e}", stored.name))?;
        let bytes = resources.iter().map(|r| r.payload.len() + r.name.len() + 32).sum::<usize>();
        let map: Arc<HashMap<u32, Arc<Resource>>> = Arc::new(resources.into_iter().map(|r| (r.id, Arc::new(r))).collect());
        let mut cache = self.cache.lock().unwrap();
        cache.clock += 1;
        let clock = cache.clock;
        if cache.files.insert(key, (map.clone(), bytes, clock)).is_none() { cache.bytes += bytes; }
        while cache.bytes > FILE_CACHE_BYTES && cache.files.len() > 1 {
            let Some((&old, _)) = cache.files.iter().filter(|(k, _)| **k != key).min_by_key(|(_, v)| v.2) else { break };
            let (_, size, _) = cache.files.remove(&old).unwrap();
            cache.bytes -= size;
        }
        Ok(map)
    }

    /// Resource `id` from whichever archive file holds it.
    pub fn get(&self, id: u32) -> Result<Arc<Resource>, String> {
        let &(a, f) = self.index.get(&id).ok_or_else(|| format!("missing resource {id:#x}"))?;
        self.file(a as usize, f as usize)?.get(&id).cloned().ok_or_else(|| format!("indexed resource {id:#x} absent from its file"))
    }

    /// A stored file of archive `archive` by name.
    pub fn find(&self, archive: usize, name: &str) -> Option<usize> {
        self.forges[archive].find(name).map(|e| e.index)
    }
}

/// Opened archives are shared between map switches: indexing all three archives takes a few seconds.
static OPEN: Mutex<Option<(std::path::PathBuf, Vec<String>, Arc<Archives>)>> = Mutex::new(None);

pub fn shared(game: &Path, archives: &[&str]) -> Result<Arc<Archives>, String> {
    let mut open = OPEN.lock().unwrap();
    if let Some((path, names, a)) = open.as_ref() {
        if path == game && names.iter().map(String::as_str).eq(archives.iter().copied()) { return Ok(a.clone()); }
    }
    let a = Arc::new(Archives::open(game, archives)?);
    *open = Some((game.to_path_buf(), archives.iter().map(|s| s.to_string()).collect(), a.clone()));
    Ok(a)
}
