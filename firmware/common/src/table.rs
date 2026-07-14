//! Cross-core double buffer for uploaded waveform tables.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU8, Ordering};

use helic_core::{WaveTable, MAX_TABLE_LEN};
use helic_proto::ErrorCode;

const NO_PENDING: u8 = 2;

struct SharedTable(UnsafeCell<WaveTable>);

// Core 0 mutates only the inactive buffer while core 1 reads only the active
// buffer. A commit marks the inactive id pending before it enters the SPSC
// command queue. Further writes remain Busy until core 1 switches at a sample
// boundary, publishes ACTIVE with release ordering, then clears PENDING.
// Therefore no mutable and shared access can target the same buffer.
unsafe impl Sync for SharedTable {}

static TABLES: [SharedTable; 2] = [
    SharedTable(UnsafeCell::new(WaveTable::empty())),
    SharedTable(UnsafeCell::new(WaveTable::empty())),
];
static ACTIVE: AtomicU8 = AtomicU8::new(0);
static PENDING: AtomicU8 = AtomicU8::new(NO_PENDING);

fn staging_id() -> Result<u8, ErrorCode> {
    if PENDING.load(Ordering::Acquire) != NO_PENDING {
        return Err(ErrorCode::Busy);
    }
    Ok(ACTIVE.load(Ordering::Acquire) ^ 1)
}

fn table(id: u8) -> &'static WaveTable {
    assert!(id < 2);
    unsafe { &*TABLES[id as usize].0.get() }
}

fn table_mut(id: u8) -> &'static mut WaveTable {
    assert!(id < 2);
    unsafe { &mut *TABLES[id as usize].0.get() }
}

pub fn set_block(offset: u32, data: &[u8]) -> Result<(), ErrorCode> {
    if !data.len().is_multiple_of(4) {
        return Err(ErrorCode::BadLength);
    }
    let offset = offset as usize;
    let count = data.len() / 4;
    if offset
        .checked_add(count)
        .is_none_or(|end| end > MAX_TABLE_LEN)
    {
        return Err(ErrorCode::BadLength);
    }
    let staging = table_mut(staging_id()?);
    for (index, raw) in data.chunks_exact(4).enumerate() {
        let value = f32::from_le_bytes(raw.try_into().unwrap());
        staging.write_block(offset + index, &[value]);
    }
    Ok(())
}

pub fn begin_commit(len: u32) -> Result<u8, ErrorCode> {
    let len = len as usize;
    if !(2..=MAX_TABLE_LEN).contains(&len) {
        return Err(ErrorCode::BadValue);
    }
    let id = staging_id()?;
    let staging = table_mut(id);
    if !staging
        .prefix(len)
        .unwrap()
        .iter()
        .all(|value| value.is_finite())
    {
        return Err(ErrorCode::BadValue);
    }
    staging.set_len(len);
    PENDING.store(id, Ordering::Release);
    Ok(id)
}

pub fn cancel_commit() {
    PENDING.store(NO_PENDING, Ordering::Release);
}

pub fn activate(id: u8) -> &'static WaveTable {
    let active = table(id);
    ACTIVE.store(id, Ordering::Release);
    PENDING.store(NO_PENDING, Ordering::Release);
    active
}

pub fn active() -> &'static WaveTable {
    table(ACTIVE.load(Ordering::Acquire))
}

pub fn active_len() -> u16 {
    active().len() as u16
}
