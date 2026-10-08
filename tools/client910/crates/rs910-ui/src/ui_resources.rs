//! Interface availability, load-on-lookup and unload.
//! Resource requests are explicit so network readiness can use this same owner.
use crate::{
    cache::{CacheError, Pack},
    ui_components::{Component, Interface, Store},
};
use anyhow::{Context, Result};
use rs910_core::fault::Fault;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
pub type Keys = Option<[i32; 4]>;
pub trait Source {
    fn capacity(&self) -> usize;
    fn ready(&mut self, id: i32) -> Result<bool>;
    fn group_capacity(&mut self, id: i32) -> Result<usize>;
    fn file(&mut self, id: i32, file: i32, keys: Keys) -> Result<Option<Vec<u8>>>;
    fn discard(&mut self, id: i32) -> Result<()>;
}
pub struct Resources {
    pub source: Box<dyn Source>,
    pub loaded: BTreeSet<i32>,
}
impl Resources {
    pub fn valid(&self, id: i32) -> Result<()> {
        anyhow::ensure!(
            id >= 0 && (id as usize) < self.source.capacity(),
            "{}",
            Fault::IndexOutOfRange.message(format_args!(
                "interface {id} outside the loaded array (capacity {})",
                self.source.capacity()
            ))
        );
        Ok(())
    }
}
pub struct PackSource {
    pack: Pack,
    index: native910::js5::ArchiveIndex,
    // JS5's unpacked file cache is shared by keyed/unkeyed requests after unpack.
    unpacked: BTreeMap<i32, BTreeMap<u32, Vec<u8>>>,
}
impl PackSource {
    pub fn new(pack: Pack) -> Result<Self> {
        let index = pack.read_archive_index("interfaces")?;
        Ok(Self {
            pack,
            index,
            unpacked: BTreeMap::new(),
        })
    }
}
impl Source for PackSource {
    fn capacity(&self) -> usize {
        self.index.group_id.last().map_or(0, |n| *n as usize + 1)
    }
    fn ready(&mut self, id: i32) -> Result<bool> {
        // Js5.isGroupValid rejects absent and zero-capacity groups before fetch.
        if id < 0
            || !self.index.group_id.contains(&(id as u32))
            || self.index.file_count_for_group(id as u32)? == 0
        {
            return Ok(false);
        }
        if self.unpacked.contains_key(&id) {
            return Ok(true);
        }
        match self.pack.read_raw_group("interfaces", id as u32) {
            Ok(_) => Ok(true),
            Err(CacheError::GroupMissing { .. } | CacheError::UnknownGroup { .. }) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }
    fn group_capacity(&mut self, id: i32) -> Result<usize> {
        let n = self.index.file_count_for_group(id as u32)?;
        Ok(if n == 0 {
            0
        } else {
            self.index.file_id_for_group_index(id as u32, n - 1)? as usize + 1
        })
    }
    fn file(&mut self, id: i32, file: i32, keys: Keys) -> Result<Option<Vec<u8>>> {
        if !self.unpacked.contains_key(&id) {
            let key = keys.map(|k| k.map(|v| v as u32));
            let data = self
                .pack
                .read_group_with_key("interfaces", id as u32, key)?;
            self.unpacked.insert(id, data);
        }
        Ok(self.unpacked[&id].get(&(file as u32)).cloned())
    }
    fn discard(&mut self, id: i32) -> Result<()> {
        self.unpacked.remove(&id);
        Ok(())
    }
}
impl Store {
    pub fn with_source(source: Box<dyn Source>) -> Self {
        Self {
            resources: Some(Resources {
                source,
                loaded: BTreeSet::new(),
            }),
            ..Self::default()
        }
    }
    pub fn from_pack(pack: Pack) -> Result<Self> {
        Ok(Self::with_source(Box::new(PackSource::new(pack)?)))
    }
    /// Opens an interface. A failed readiness probe clears the
    /// global interface slot. Existing arrays are reused when reloading partial
    /// interfaces; decoded component references are not replaced.
    pub fn open(&mut self, id: i32, keys: Keys) -> Result<bool> {
        let Some(r) = self.resources.as_mut() else {
            return Ok(self.interfaces.contains_key(&id));
        };
        r.valid(id)?;
        if r.loaded.contains(&id) {
            return Ok(true);
        }
        if !r.source.ready(id)? {
            self.interfaces.remove(&id);
            return Ok(false);
        }
        let count = r.source.group_capacity(id)?;
        let old = self.interfaces.get(&id).cloned();
        let interface = old
            .clone()
            .unwrap_or_else(|| Interface::new(vec![None; count]));
        // The component array is replaced only for an empty group.
        if count == 0 {
            interface.borrow_mut().components = Rc::new(RefCell::new(vec![]));
        }
        interface.borrow_mut().transient = false;
        for file in 0..count {
            let array = interface.borrow().components.clone();
            let missing = array
                .borrow()
                .get(file)
                .context("partial interface array index")?
                .is_none();
            if missing {
                if let Some(b) = r.source.file(id, file as i32, keys)? {
                    let component = Component::decode((id << 16).wrapping_add(file as i32), &b)?;
                    array.borrow_mut()[file] = Some(Rc::new(RefCell::new(component)));
                }
            }
        }
        self.interfaces.insert(id, interface);
        r.loaded.insert(id);
        Ok(true)
    }
    /// Discards an interface: discard is conditional on the loaded bit, not on
    /// whether an interface object remains reachable elsewhere.
    pub fn discard_if_unloaded(&mut self, id: i32) -> Result<()> {
        if id == -1 {
            return Ok(());
        }
        if let Some(r) = &mut self.resources {
            r.valid(id)?;
            if r.loaded.contains(&id) {
                return Ok(());
            }
            r.source.discard(id)?;
        }
        self.interfaces.remove(&id);
        Ok(())
    }
    /// Replaces the global
    /// interface and loaded-bit arrays, dropping every decoded interface.
    /// The JS5 archive's unpacked cache is a separate owner and is kept.
    /// `showLogin`/`showLobby` call this after
    /// closing the previous tree.
    pub fn reset_all(&mut self) {
        if let Some(r) = &mut self.resources {
            r.loaded.clear();
        }
        self.interfaces.clear();
    }
    /// Unloads an interface. Detached active/drag references retain their objects.
    pub fn unload(&mut self, id: i32) -> Result<()> {
        if let Some(r) = &mut self.resources {
            r.valid(id)?;
            r.loaded.remove(&id);
        }
        self.discard_if_unloaded(id)
    }
}
