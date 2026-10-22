//! The CPU threads every parallel stage runs on.
//!
//! Rayon's global pool takes every core, and a develop that holds every core
//! makes the whole system lag. Core work runs on this pool instead: at most
//! [`Workers::MAX_THREADS`] threads, and always one core left free.

use std::sync::OnceLock;

/// The capped pool.
pub struct Workers;

impl Workers {
    /// Most threads a develop uses, however many cores there are.
    pub const MAX_THREADS: usize = 4;

    /// Threads in the pool: one fewer than the cores, from 1 to
    /// [`Self::MAX_THREADS`].
    pub fn threads() -> usize {
        std::thread::available_parallelism()
            .map_or(1, |cores| cores.get().saturating_sub(1))
            .clamp(1, Self::MAX_THREADS)
    }

    /// Run `op` on the pool, so its rayon work, rawler's included, stays
    /// on these threads. Where no pool can start (wasm without threads),
    /// `op` runs here.
    pub fn install<R: Send>(op: impl FnOnce() -> R + Send) -> R {
        match Self::pool() {
            Some(pool) => pool.install(op),
            None => op(),
        }
    }

    /// `op` on a pool of its own, `threads` wide and capped the same way.
    ///
    /// For background work that should not queue ahead of a preview on
    /// the shared pool, such as a demosaic that runs while the first
    /// preview renders.
    pub fn install_separate<R: Send>(threads: usize, op: impl FnOnce() -> R + Send) -> R {
        match rayon::ThreadPoolBuilder::new()
            .num_threads(threads.clamp(1, Self::threads()))
            .thread_name(|index| format!("rawstock-background-{index}"))
            .build()
        {
            Ok(pool) => pool.install(op),
            Err(_) => op(),
        }
    }

    fn pool() -> Option<&'static rayon::ThreadPool> {
        static POOL: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();
        POOL.get_or_init(|| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(Self::threads())
                .thread_name(|index| format!("rawstock-worker-{index}"))
                .build()
                .ok()
        })
        .as_ref()
    }
}
