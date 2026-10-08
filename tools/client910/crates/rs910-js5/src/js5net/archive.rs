//! Archive readiness, named group lookup and download progress.

use std::sync::Arc;

use super::{name_hash, Js5Index, Net, NetResourceProvider};

/// `Js5`: the readiness and percentage half. The group
/// bytes themselves reach this port's consumers through
/// [`crate::cache::Pack`], which reads the same disk stores the provider
/// verified and wrote; `packed[g]` therefore records that the group arrived.
#[allow(dead_code)] // `archive`/`discard*` are kept for parity; see the doc above.
pub struct Js5 {
    pub archive: u32,
    index: Option<Arc<Js5Index>>,
    packed: Vec<bool>,
    /// `discardPacked` / `discardUnpacked`.
    pub discard_packed: bool,
    pub discard_unpacked: i32,
}

impl Js5 {
    #[must_use]
    pub fn new(archive: u32, discard_packed: bool, discard_unpacked: i32) -> Self {
        Self {
            archive,
            index: None,
            packed: Vec::new(),
            discard_packed,
            discard_unpacked,
        }
    }

    /// Whether the archive index is available.
    pub fn is_index_ready(&mut self, p: &mut NetResourceProvider, net: &mut Net) -> bool {
        if self.index.is_none() {
            let Some(index) = p.fetch_index(net) else {
                return false;
            };
            self.packed = vec![false; index.index.capacity];
            self.index = Some(index);
        }
        true
    }

    /// `Js5.getChecksum`: the checksum of the archive index, once the index
    /// is available.
    pub fn checksum(&mut self, p: &mut NetResourceProvider, net: &mut Net) -> Option<i32> {
        self.is_index_ready(p, net)
            .then(|| self.index.as_ref().expect("index").crc)
    }

    /// Whether the group exists in the index.
    pub fn is_group_valid(
        &mut self,
        p: &mut NetResourceProvider,
        net: &mut Net,
        group: i32,
    ) -> bool {
        if !self.is_index_ready(p, net) {
            return false;
        }
        let caps = &self.index.as_ref().expect("index").index.group_capacities;
        usize::try_from(group).is_ok_and(|g| g < caps.len() && caps[g] != 0)
    }

    /// Whether the file exists in the group.
    pub fn is_file_valid(
        &mut self,
        p: &mut NetResourceProvider,
        net: &mut Net,
        group: i32,
        file: i32,
    ) -> bool {
        if !self.is_index_ready(p, net) {
            return false;
        }
        let caps = &self.index.as_ref().expect("index").index.group_capacities;
        match (usize::try_from(group), u32::try_from(file)) {
            (Ok(g), Ok(f)) => g < caps.len() && f < caps[g],
            _ => false,
        }
    }

    /// Fetch a group through the provider and record whether it is packed.
    pub(super) fn fetch_group(&mut self, p: &mut NetResourceProvider, net: &mut Net, group: i32) {
        let ready = p.fetch_group(net, group);
        if let Some(slot) = self.packed.get_mut(group as usize) {
            *slot = ready;
        }
    }

    /// Queue a group for prefetching.
    #[allow(dead_code)]
    pub fn prefetch_group(p: &mut NetResourceProvider, group: i32) {
        p.prefetch_group(group);
    }

    /// Request a file's group; true when it is ready.
    pub fn request_download(
        &mut self,
        p: &mut NetResourceProvider,
        net: &mut Net,
        group: i32,
        file: i32,
    ) -> bool {
        if !self.is_file_valid(p, net, group, file) {
            return false;
        }
        if !self.packed[group as usize] {
            self.fetch_group(p, net, group);
        }
        self.packed[group as usize]
    }

    /// Load a file by its flat id; true when it is ready.
    pub fn load_file(&mut self, p: &mut NetResourceProvider, net: &mut Net, id: i32) -> bool {
        if !self.is_index_ready(p, net) {
            return false;
        }
        let caps = self
            .index
            .as_ref()
            .expect("index")
            .index
            .group_capacities
            .clone();
        if caps.len() == 1 {
            return self.request_download(p, net, 0, id);
        }
        if !self.is_group_valid(p, net, id) {
            return false;
        }
        caps[id as usize] == 1 && self.request_download(p, net, id, 0)
    }

    /// Whether the group is downloaded.
    pub fn is_group_ready(
        &mut self,
        p: &mut NetResourceProvider,
        net: &mut Net,
        group: i32,
    ) -> bool {
        if !self.is_group_valid(p, net, group) {
            return false;
        }
        if !self.packed[group as usize] {
            self.fetch_group(p, net, group);
        }
        self.packed[group as usize]
    }

    /// Fetch every group; true once all are downloaded.
    pub fn fetch_all(&mut self, p: &mut NetResourceProvider, net: &mut Net) -> bool {
        if !self.is_index_ready(p, net) {
            return false;
        }
        let ids = self.index.as_ref().expect("index").index.group_id.clone();
        let mut all = true;
        for group in ids {
            let g = group as usize;
            if !self.packed[g] {
                self.fetch_group(p, net, group as i32);
                if !self.packed[g] {
                    all = false;
                }
            }
        }
        all
    }

    /// Download progress of one group, in percent.
    pub fn group_percentage(
        &mut self,
        p: &mut NetResourceProvider,
        net: &mut Net,
        group: i32,
    ) -> i32 {
        if !self.is_group_valid(p, net, group) {
            return 0;
        }
        if self.packed[group as usize] {
            100
        } else {
            p.group_percentage(group)
        }
    }

    /// Download progress of the whole archive, in percent.
    pub fn percentage(&mut self, p: &mut NetResourceProvider, net: &mut Net) -> i32 {
        if !self.is_index_ready(p, net) {
            return 0;
        }
        let sizes = self
            .index
            .as_ref()
            .expect("index")
            .index
            .group_sizes
            .clone();
        let mut total = 0_i64;
        let mut done = 0_i64;
        for (g, &size) in sizes[..self.packed.len()].iter().enumerate() {
            if size > 0 {
                total += 100;
                done += i64::from(self.group_percentage(p, net, g as i32));
            }
        }
        if total == 0 {
            100
        } else {
            (done * 100 / total) as i32
        }
    }

    /// `isFileReady(id)`: the id names a valid file.
    #[allow(dead_code)]
    pub fn is_file_ready(&mut self, p: &mut NetResourceProvider, net: &mut Net, id: i32) -> bool {
        if !self.is_index_ready(p, net) {
            return false;
        }
        let caps = self
            .index
            .as_ref()
            .expect("index")
            .index
            .group_capacities
            .clone();
        if caps.len() == 1 {
            return self.is_file_valid(p, net, 0, id);
        }
        if !self.is_group_valid(p, net, id) {
            return false;
        }
        caps[id as usize] == 1 && self.is_file_valid(p, net, id, 0)
    }

    /// `groupNameHashTable.get(hash)`: the first group carrying the hash.
    pub(super) fn group_by_hash(&self, hash: i32) -> i32 {
        self.index
            .as_ref()
            .and_then(|i| i.index.group_name_hashes.as_ref())
            .and_then(|names| names.iter().position(|&h| h == hash))
            .map_or(-1, |g| g as i32)
    }

    /// The file of a group whose name hash matches, or -1.
    pub(super) fn file_by_hash(&self, group: i32, hash: i32) -> i32 {
        self.index
            .as_ref()
            .and_then(|i| i.index.file_name_hashes.as_ref())
            .and_then(|tables| tables.get(usize::try_from(group).ok()?))
            .and_then(|table| table.iter().position(|&h| h == hash))
            .map_or(-1, |f| f as i32)
    }

    /// The id of the group with this name, or -1.
    pub fn group_id(&mut self, p: &mut NetResourceProvider, net: &mut Net, name: &str) -> i32 {
        if !self.is_index_ready(p, net) {
            return -1;
        }
        let group = self.group_by_hash(name_hash(name));
        if self.is_group_valid(p, net, group) {
            group
        } else {
            -1
        }
    }

    /// Whether the group and file names resolve.
    pub fn is_file_name_valid(
        &mut self,
        p: &mut NetResourceProvider,
        net: &mut Net,
        group: &str,
        file: &str,
    ) -> bool {
        if !self.is_index_ready(p, net) {
            return false;
        }
        let g = self.group_by_hash(name_hash(group));
        g >= 0 && self.file_by_hash(g, name_hash(file)) >= 0
    }

    /// `request_download(group, file)` by names.
    pub fn request_download_named(
        &mut self,
        p: &mut NetResourceProvider,
        net: &mut Net,
        group: &str,
        file: &str,
    ) -> bool {
        if !self.is_index_ready(p, net) {
            return false;
        }
        let g = self.group_by_hash(name_hash(group));
        if !self.is_group_valid(p, net, g) {
            return false;
        }
        let f = self.file_by_hash(g, name_hash(file));
        self.request_download(p, net, g, f)
    }

    /// Request a group by name.
    pub fn request_group(
        &mut self,
        p: &mut NetResourceProvider,
        net: &mut Net,
        name: &str,
    ) -> bool {
        if self.group_id(p, net, "") == -1 {
            self.request_download_named(p, net, name, "")
        } else {
            self.request_download_named(p, net, "", name)
        }
    }

    /// Whether the named group is downloaded.
    pub fn is_group_ready_named(
        &mut self,
        p: &mut NetResourceProvider,
        net: &mut Net,
        name: &str,
    ) -> bool {
        if !self.is_index_ready(p, net) {
            return false;
        }
        let g = self.group_by_hash(name_hash(name));
        self.is_group_ready(p, net, g)
    }

    /// Download progress of a named group, in percent.
    pub fn group_percentage_named(
        &mut self,
        p: &mut NetResourceProvider,
        net: &mut Net,
        name: &str,
    ) -> i32 {
        if !self.is_index_ready(p, net) {
            return 0;
        }
        let g = self.group_by_hash(name_hash(name));
        self.group_percentage(p, net, g)
    }
}
