//! All I/O lives here: embedded assets, config discovery + merge + include
//! resolution, target-file globbing, and the `oneharness` subprocess client.
//! The domain layer ([`crate::domain`]) stays pure.

pub mod assets;
pub mod configfs;
pub mod files;
pub mod oneharness;
