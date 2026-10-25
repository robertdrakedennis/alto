//! Resolve at the call site. Missing/one-byte files and failed decodes are not
//! cached; successful gets promote entries in the 128-entry script cache.
use crate::{
    cache::{CacheError, Pack},
    entity_runtime::bits_pack::Inputs,
};
use anyhow::{Context, Result};
use native910::{
    opcode::OpcodeBook,
    script::{CompiledScript, Operand},
    vm::{ScriptProvider, VmError, VmResult},
};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

/// A script group holds its bytecode in file zero.
const SCRIPT_FILE: u32 = 0;

pub trait Source {
    fn file(&mut self, id: i32) -> Result<Option<Vec<u8>>>;
    /// The group whose name hash is `hash` (trigger scripts are found by
    /// their packed trigger and key, which the index stores as the group's
    /// name hash); `None` when no group carries it.
    fn group_by_hash(&mut self, hash: i32) -> Option<i32> {
        let _ = hash;
        None
    }
}
pub struct PackSource {
    pack: Pack,
    /// Group id by name hash (the first group of a hash wins).
    by_hash: BTreeMap<i32, i32>,
}
impl PackSource {
    pub fn new(pack: Pack) -> Result<Self> {
        let index = pack.read_archive_index("scripts")?;
        let mut by_hash = BTreeMap::new();
        for (group, hash) in index.group_name_hashes.iter().flatten().enumerate() {
            if *hash != -1 {
                by_hash.entry(*hash).or_insert(group as i32);
            }
        }
        Ok(Self { pack, by_hash })
    }
}
impl Source for PackSource {
    fn group_by_hash(&mut self, hash: i32) -> Option<i32> {
        self.by_hash.get(&hash).copied()
    }
    fn file(&mut self, id: i32) -> Result<Option<Vec<u8>>> {
        let Ok(id) = u32::try_from(id) else {
            return Ok(None);
        };
        match self.pack.read_group("scripts", id) {
            Ok(mut files) => Ok(files.remove(&SCRIPT_FILE)),
            Err(CacheError::GroupMissing { .. } | CacheError::UnknownGroup { .. }) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}
/// Decoded scripts kept in memory.
const SCRIPT_CACHE_SIZE: usize = 128;
type Cache = rs910_core::lru::Lru<i32, Rc<CompiledScript>, SCRIPT_CACHE_SIZE>;
/// What fetching a script group found.
enum Lookup {
    Script(Rc<CompiledScript>),
    /// The file is absent.
    Missing,
    /// The file is a single byte: a placeholder that means no script.
    Stub,
}
pub struct Scripts {
    source: RefCell<Box<dyn Source>>,
    book: OpcodeBook,
    cache: RefCell<Cache>,
}
impl Scripts {
    pub fn new(source: Box<dyn Source>) -> Result<Self> {
        Ok(Self {
            source: RefCell::new(source),
            book: OpcodeBook::embedded()?,
            cache: RefCell::new(Cache::default()),
        })
    }
    pub fn from_pack(pack: Pack) -> Result<Self> {
        Self::new(Box::new(PackSource::new(pack)?))
    }
    #[cfg(any(test, feature = "test-hooks"))] // test-only helpers
    pub fn clear(&self) {
        *self.cache.borrow_mut() = Cache::default();
    }
    /// The binding callback runs only on decode, which captures
    /// variable-definition objects at load time.
    pub fn get(
        &self,
        id: i32,
        bind: impl FnOnce(&CompiledScript) -> Result<()>,
    ) -> Result<Option<Rc<CompiledScript>>> {
        Ok(match self.lookup(id, bind)? {
            Lookup::Script(script) => Some(script),
            Lookup::Missing | Lookup::Stub => None,
        })
    }

    /// Fetches group `id`: the cached script, a decode, or why there is none.
    fn lookup(&self, id: i32, bind: impl FnOnce(&CompiledScript) -> Result<()>) -> Result<Lookup> {
        {
            if let Some(script) = self.cache.borrow_mut().get(&id).cloned() {
                return Ok(Lookup::Script(script));
            }
        }
        let bytes = self.source.borrow_mut().file(id)?;
        let Some(bytes) = bytes else {
            return Ok(Lookup::Missing);
        };
        if bytes.len() <= 1 {
            return Ok(Lookup::Stub);
        }
        let script = native910::script::decode_script(&bytes, &self.book)
            .with_context(|| format!("decode client script {id}"))?;
        bind(&script).with_context(|| format!("bind client script {id}"))?;
        let script = Rc::new(script);
        self.cache.borrow_mut().insert(id, script.clone());
        Ok(Lookup::Script(script))
    }

    /// `getByTrigger`: trigger scripts live in the same JS5 archive under a
    /// packed trigger/key name hash (not a group id), with a key-specific,
    /// type-specific and generic fallback in that order.
    pub fn get_trigger(
        &self,
        trigger: i32,
        primary: i32,
        secondary: i32,
        mut bind: impl FnMut(&CompiledScript) -> Result<()>,
    ) -> Result<Option<(i32, Rc<CompiledScript>)>> {
        let ids = [
            trigger | primary.wrapping_shl(10),
            trigger | secondary.wrapping_add(65_536).wrapping_shl(10),
            trigger | 0x03ff_fc00,
        ];
        for hash in ids {
            let group = self.source.borrow_mut().group_by_hash(hash);
            let Some(group) = group else { continue };
            // Only a missing file falls through to the next candidate; a
            // one-byte file ends the search without a script.
            match self.lookup(group, |script| bind(script))? {
                Lookup::Script(script) => return Ok(Some((group, script))),
                Lookup::Stub => return Ok(None),
                Lookup::Missing => {}
            }
        }
        Ok(None)
    }
}
pub struct Provider<'a> {
    pub scripts: &'a Scripts,
    pub definitions: &'a Inputs,
}
impl Provider<'_> {
    pub fn get(&self, id: i32) -> Result<Option<Rc<CompiledScript>>> {
        self.scripts.get(id, |script| {
            for ins in &script.code {
                match &ins.operand {
                    Operand::VarRef(v) => {
                        self.definitions
                            .binding(v.domain as u8, v.id as i32)
                            .map_err(|e| anyhow::anyhow!("{e:?}"))?
                            .context("variable definition domain is not installed")?;
                    }
                    Operand::VarBitRef(v) => {
                        self.definitions
                            .get(v.id as i32, false)
                            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
                    }
                    _ => {}
                }
            }
            Ok(())
        })
    }

    pub fn get_trigger(
        &self,
        trigger: i32,
        primary: i32,
        secondary: i32,
    ) -> Result<Option<(i32, Rc<CompiledScript>)>> {
        self.scripts
            .get_trigger(trigger, primary, secondary, |script| {
                for ins in &script.code {
                    match &ins.operand {
                        Operand::VarRef(v) => {
                            self.definitions
                                .binding(v.domain as u8, v.id as i32)
                                .map_err(|e| anyhow::anyhow!("{e:?}"))?
                                .context("variable definition domain is not installed")?;
                        }
                        Operand::VarBitRef(v) => {
                            self.definitions
                                .get(v.id as i32, false)
                                .map_err(|e| anyhow::anyhow!("{e:?}"))?;
                        }
                        _ => {}
                    }
                }
                Ok(())
            })
    }
}
impl ScriptProvider for Provider<'_> {
    fn resolve(&self, id: i32) -> VmResult<Option<CompiledScript>> {
        self.get(id)
            .map(|s| s.map(|s| (*s).clone()))
            .map_err(|e| VmError::TrapFailed {
                command: format!("load script {id}"),
                reason: format!("{e:#}"),
            })
    }
}
