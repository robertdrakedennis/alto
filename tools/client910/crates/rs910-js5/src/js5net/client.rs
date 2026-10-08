//! Master-index loading and archive resource-provider coordination.

use std::sync::Arc;

use anyhow::Context;

use crate::cache::{DiskStore, Stored};

use super::{MasterIndex, Net, NetResourceProvider, ProviderSpec, RequestRef};

// ---------------------------------------------------------------------------
// Js5Client
// ---------------------------------------------------------------------------

/// The JS5 client: master index, per-archive providers and their update.
pub struct Js5Client {
    master_index_request: Option<RequestRef>,
    http_master_index_request: Option<RequestRef>,
    pub master_index: Option<MasterIndex>,
    http_master_index: Option<MasterIndex>,
    http_index_applied: bool,
    /// `resourceProviders`, allocated with the first master index.
    pub providers: Option<Vec<Option<NetResourceProvider>>>,
}

impl Js5Client {
    /// The constructor: request `255/255` over TCP and HTTP.
    pub fn new(net: &mut Net) -> Self {
        let master_index_request =
            (!net.tcp.is_urgents_full()).then(|| net.tcp.queue_request(255, 255, 0, true));
        let http_master_index_request = net.http.request_master_index();
        Self {
            master_index_request,
            http_master_index_request,
            master_index: None,
            http_master_index: None,
            http_index_applied: false,
            providers: None,
        }
    }

    /// Advance the master index load; true once it is loaded.
    pub fn load_master_index(&mut self, net: &mut Net) -> anyhow::Result<bool> {
        if self.master_index.is_some() {
            return Ok(true);
        }
        if self.master_index_request.is_none() {
            if net.tcp.is_urgents_full() {
                return Ok(false);
            }
            self.master_index_request = Some(net.tcp.queue_request(255, 255, 0, true));
        }
        let request = self
            .master_index_request
            .clone()
            .expect("master index request");
        if request.borrow().incomplete() {
            return Ok(false);
        }
        let Some(Stored::Bytes(bytes)) = request.borrow().stored() else {
            anyhow::bail!("js5 master index request completed without bytes");
        };
        let master = MasterIndex::decode(&bytes)?;
        match self.providers.as_mut() {
            None => {
                self.providers = Some((0..master.archives.len()).map(|_| None).collect());
            }
            Some(providers) => {
                for (i, provider) in providers.iter_mut().enumerate() {
                    if let (Some(provider), Some(data)) =
                        (provider.as_mut(), master.archives.get(i))
                    {
                        provider.request_index(net, data.crc, data.whirlpool, data.version);
                        if provider.has_http_client() {
                            net.http.set_http_enabled(false);
                        }
                    }
                }
            }
        }
        self.master_index = Some(master);
        self.http_index_applied = false;
        Ok(true)
    }

    /// The provider of an archive, created on first use.
    pub fn archive_provider(
        &mut self,
        net: &mut Net,
        archive: u32,
        datafs: Option<Arc<DiskStore>>,
        masterfs: Option<Arc<DiskStore>>,
        discard_orphans: bool,
        use_http: bool,
    ) -> anyhow::Result<&mut NetResourceProvider> {
        let master = self.master_index.as_ref().context("js5: no master index")?;
        let providers = self.providers.as_mut().context("js5: no providers")?;
        let slot = providers
            .get_mut(archive as usize)
            .with_context(|| format!("js5: archive {archive} outside the master index"))?;
        if slot.is_none() {
            let data = &master.archives[archive as usize];
            let provider = NetResourceProvider::new(
                ProviderSpec {
                    archive,
                    datafs,
                    masterfs,
                    http: use_http,
                    discard_orphans,
                    crc: data.crc,
                    whirlpool: data.whirlpool,
                    index_version: data.version,
                },
                net.disk,
            );
            if let (Some(http_master), true) = (&self.http_master_index, use_http) {
                if let Some(data) = http_master.archives.get(archive as usize) {
                    let up_to_date =
                        provider.is_index_up_to_date(data.crc, &data.whirlpool, data.version);
                    net.http.set_http_enabled(up_to_date);
                }
            }
            *slot = Some(provider);
        }
        Ok(slot.as_mut().expect("provider"))
    }

    /// Advance every provider.
    pub fn update(&mut self, net: &mut Net) -> anyhow::Result<()> {
        let Some(providers) = self.providers.as_mut() else {
            return Ok(());
        };
        for provider in providers.iter_mut().flatten() {
            provider.process_prefetch_queue(net);
        }
        for provider in providers.iter_mut().flatten() {
            provider.update(net);
        }
        if self.master_index.is_none() {
            self.load_master_index(net)?;
        } else if !self.http_index_applied {
            match self.http_master_index_request.clone() {
                None => self.http_master_index_request = net.http.request_master_index(),
                Some(request) if !request.borrow().incomplete() => {
                    let bytes = match request.borrow().stored() {
                        Some(Stored::Bytes(bytes)) => bytes,
                        _ => Vec::new(),
                    };
                    let providers = self.providers.as_ref();
                    match MasterIndex::decode(&bytes) {
                        Ok(master) => {
                            for provider in providers.into_iter().flatten().flatten() {
                                if provider.has_http_client() {
                                    if let Some(data) =
                                        master.archives.get(provider.archive as usize)
                                    {
                                        let ok = provider.is_index_up_to_date(
                                            data.crc,
                                            &data.whirlpool,
                                            data.version,
                                        );
                                        net.http.set_http_enabled(ok);
                                    }
                                }
                            }
                            self.http_master_index = Some(master);
                        }
                        Err(_) => {
                            for provider in providers.into_iter().flatten().flatten() {
                                if provider.has_http_client() {
                                    net.http.set_http_enabled(false);
                                }
                            }
                        }
                    }
                    self.http_master_index_request = None;
                    self.http_index_applied = true;
                }
                Some(_) => {}
            }
        }
        Ok(())
    }

    pub fn provider_mut(&mut self, archive: u32) -> Option<&mut NetResourceProvider> {
        self.providers.as_mut()?.get_mut(archive as usize)?.as_mut()
    }
}
