//! `rs910-js5`: the cache and JS5 network layer of the 910 client port
//! (`docs/architecture.md`). The JS5 cache, network and
//! loadable-resource layers.
//!
//! - [`cache`]: the on-disk `.js5` pack reader ([`cache::Pack`], one shared
//!   handle whose clones share the decoded-index cache; overlay master-index
//!   writes bump a generation that invalidates it), XTEA keys, the
//!   `--cache-dir` [`cache::DiskOverlay`] and [`cache::DiskStore`].
//! - [`js5net`]: the JS5 TCP/HTTP clients, `Js5Client`, the net resource
//!   providers with prefetch, the disk cache worker and [`js5net::Js5System`]
//!   (the JS5 statics).
//!
//! Modules keep their client910 names, so the facade in
//! `client910/src/lib.rs` keeps `crate::cache::...` and `crate::js5net::...`
//! paths compiling (tools/README.md "Crate conventions").

pub mod cache;
pub mod js5_fetch;
pub mod js5net;
#[cfg(any(test, feature = "test-hooks"))]
pub mod test_support;
