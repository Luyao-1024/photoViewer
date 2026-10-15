//! Short local publication exclusion. Network and preparation never hold it.
//! Readers already decoding the previous version finish before replacement;
//! nested metadata/decoder calls on the same thread share their outer lease.
use crate::core::error::{AppError, Result};
use std::{
    cell::Cell,
    sync::{RwLock, RwLockReadGuard, RwLockWriteGuard},
};
static FILES: RwLock<()> = RwLock::new(());
thread_local! {static READS:Cell<usize>=const{Cell::new(0)};static WRITES:Cell<usize>=const{Cell::new(0)};}
pub struct ReadLease {
    _guard: Option<RwLockReadGuard<'static, ()>>,
}
pub struct WriteLease {
    _guard: Option<RwLockWriteGuard<'static, ()>>,
}
pub fn read() -> Result<ReadLease> {
    let nested = READS.with(|n| n.get() > 0) || WRITES.with(|n| n.get() > 0);
    let guard = if nested {
        None
    } else {
        Some(
            FILES
                .read()
                .map_err(|_| AppError::Backend("publication read protection failed".into()))?,
        )
    };
    READS.with(|n| n.set(n.get() + 1));
    Ok(ReadLease { _guard: guard })
}
pub fn write() -> Result<WriteLease> {
    if READS.with(|n| n.get() > 0) && !WRITES.with(|n| n.get() > 0) {
        return Err(AppError::Backend(
            "cannot publish while holding a media read lease".into(),
        ));
    }
    let guard = if WRITES.with(|n| n.get() > 0) {
        None
    } else {
        Some(
            FILES
                .write()
                .map_err(|_| AppError::Backend("publication write protection failed".into()))?,
        )
    };
    WRITES.with(|n| n.set(n.get() + 1));
    Ok(WriteLease { _guard: guard })
}
impl Drop for ReadLease {
    fn drop(&mut self) {
        READS.with(|n| n.set(n.get().saturating_sub(1)));
    }
}
impl Drop for WriteLease {
    fn drop(&mut self) {
        WRITES.with(|n| n.set(n.get().saturating_sub(1)));
    }
}
