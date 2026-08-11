//! Owner-checked cross-core double buffers for large real-time values.

use core::cell::{Cell, UnsafeCell};
use core::marker::PhantomData;
use core::sync::atomic::{AtomicU8, Ordering};

use crate::WaveTable;

// Zero must mean idle so zero-valued buffers remain in `.bss` when held by a
// `ConstStaticCell`. Pending bank ids are therefore encoded as bank + 1.
const NO_PENDING: u8 = 0;

/// A buffer commit cannot begin while an earlier commit awaits activation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferError {
    Busy,
}

/// Linear proof that one bank activation is outstanding.
///
/// The token is deliberately neither [`Copy`] nor [`Clone`]. It moves from
/// [`Staging::commit`] through the cross-core command queue and is consumed by
/// exactly one of [`Active::activate`] or [`Staging::cancel`]. Its erased value
/// type lets the non-generic command envelope carry table or force-vector
/// commits; the owner address prevents cross-buffer replay.
#[derive(Debug)]
pub struct CommitToken {
    owner: usize,
    bank: u8,
}

/// Two banks shared through uniquely owned staging and active endpoints.
pub struct DoubleBuffer<T> {
    banks: [UnsafeCell<T>; 2],
    // Written only by the active endpoint during activation.
    active: AtomicU8,
    // Whole-word stores publish bank + 1, or NO_PENDING.
    pending: AtomicU8,
}

/// Waveform-table buffer retained as the convenient default specialisation.
pub type TableBuffer = DoubleBuffer<WaveTable>;
/// Buffer for an atomic fixed-width force or parameter vector.
pub type ValueBuffer<const N: usize> = DoubleBuffer<[f32; N]>;
/// Core-1 endpoint for waveform tables.
pub type ActiveTable = Active<WaveTable>;
/// Core-1 endpoint for fixed-width value vectors.
pub type ActiveValues<const N: usize> = Active<[f32; N]>;
/// Core-0 endpoint for fixed-width value vectors.
pub type ValueStaging<const N: usize> = Staging<[f32; N]>;

// SAFETY: `split` yields one non-Sync endpoint for each core. `Staging` mutates
// only the inactive bank while no commit is pending; `Active` reads only its
// cached active bank. The Release/Acquire protocol makes staged writes visible
// before activation and the new active id visible before staging resumes.
unsafe impl<T: Send> Sync for DoubleBuffer<T> {}

impl<T: 'static> DoubleBuffer<T> {
    const fn from_banks(first: T, second: T) -> Self {
        Self {
            banks: [UnsafeCell::new(first), UnsafeCell::new(second)],
            active: AtomicU8::new(0),
            pending: AtomicU8::new(NO_PENDING),
        }
    }

    /// Split this buffer exactly once into its two uniquely owned endpoints.
    ///
    /// A `ConstStaticCell<TableBuffer>` supplies the required static mutable
    /// reference without constructing the 32 KiB value on the firmware stack.
    ///
    /// ```compile_fail
    /// use helic_core::TableBuffer;
    /// use static_cell::ConstStaticCell;
    ///
    /// static BUFFER: ConstStaticCell<TableBuffer> =
    ///     ConstStaticCell::new(TableBuffer::new());
    /// let buffer = BUFFER.take();
    /// let _first = buffer.split();
    /// let _second = buffer.split();
    /// ```
    pub fn split(&'static mut self) -> (Staging<T>, Active<T>) {
        let buf: &'static DoubleBuffer<T> = self;
        (
            Staging {
                buf,
                _not_sync: PhantomData,
            },
            Active {
                buf,
                current: 0,
                _not_sync: PhantomData,
            },
        )
    }

    #[inline]
    fn identity(&self) -> usize {
        self as *const Self as usize
    }
}

impl DoubleBuffer<WaveTable> {
    pub const fn new() -> Self {
        Self::from_banks(WaveTable::empty(), WaveTable::empty())
    }
}

impl<const N: usize> DoubleBuffer<[f32; N]> {
    /// Construct two zeroed value banks suitable for static `.bss` storage.
    pub const fn values() -> Self {
        Self::from_banks([0.0; N], [0.0; N])
    }
}

impl Default for DoubleBuffer<WaveTable> {
    fn default() -> Self {
        Self::new()
    }
}

/// Core-0 ownership of an inactive bank and commit publication.
///
/// This endpoint is `Send` but deliberately not `Sync`.
pub struct Staging<T: 'static = WaveTable> {
    buf: &'static DoubleBuffer<T>,
    _not_sync: PhantomData<Cell<()>>,
}

impl<T: 'static> Staging<T> {
    /// Borrow the inactive bank while no earlier commit is pending.
    ///
    /// The mutable borrow is tied to `self`, preventing two staging borrows.
    ///
    /// ```compile_fail
    /// use helic_core::TableBuffer;
    /// use static_cell::ConstStaticCell;
    ///
    /// static BUFFER: ConstStaticCell<TableBuffer> =
    ///     ConstStaticCell::new(TableBuffer::new());
    /// let (mut staging, _) = BUFFER.take().split();
    /// let first = staging.buffer().unwrap();
    /// let second = staging.buffer().unwrap();
    /// first.write_block(0, &[1.0, 2.0]);
    /// second.write_block(0, &[3.0, 4.0]);
    /// ```
    pub fn buffer(&mut self) -> Result<&mut T, BufferError> {
        if self.buf.pending.load(Ordering::Acquire) != NO_PENDING {
            return Err(BufferError::Busy);
        }
        let bank = self.buf.active.load(Ordering::Acquire) ^ 1;
        // SAFETY: no commit is pending, so `bank` is inactive. This uniquely
        // owned endpoint and the `&mut self` borrow prevent a second mutable
        // reference; the active endpoint reads only the other bank.
        Ok(unsafe { &mut *self.buf.banks[bank as usize].get() })
    }

    /// Publish the staged bank and return its linear activation token.
    pub fn commit(&mut self) -> Result<CommitToken, BufferError> {
        if self.buf.pending.load(Ordering::Relaxed) != NO_PENDING {
            return Err(BufferError::Busy);
        }
        let bank = self.buf.active.load(Ordering::Relaxed) ^ 1;
        // Release makes all preceding writes through `buffer` visible to the
        // active endpoint's Acquire load before it reads the new bank.
        self.buf.pending.store(bank + 1, Ordering::Release);
        Ok(CommitToken {
            owner: self.buf.identity(),
            bank,
        })
    }

    /// Cancel a returned commit token, ignoring tokens from another buffer.
    pub fn cancel(&mut self, token: CommitToken) {
        if token.owner == self.buf.identity() {
            self.buf.pending.store(NO_PENDING, Ordering::Release);
        }
    }
}

/// Core-1 ownership of the bank used by the real-time loop.
///
/// This endpoint is `Send` but deliberately not `Sync`.
pub struct Active<T: 'static = WaveTable> {
    buf: &'static DoubleBuffer<T>,
    current: u8,
    _not_sync: PhantomData<Cell<()>>,
}

impl<T: 'static> Active<T> {
    /// Borrow the cached active bank without an atomic operation per tick.
    ///
    /// Holding this borrow prevents activation through the same endpoint.
    ///
    /// ```compile_fail
    /// use helic_core::TableBuffer;
    /// use static_cell::ConstStaticCell;
    ///
    /// static BUFFER: ConstStaticCell<TableBuffer> =
    ///     ConstStaticCell::new(TableBuffer::new());
    /// let (mut staging, mut active) = BUFFER.take().split();
    /// staging.buffer().unwrap().write_block(0, &[1.0, 2.0]);
    /// let token = staging.commit().unwrap();
    /// let table = active.get();
    /// active.activate(token);
    /// let _ = table.len();
    /// ```
    #[inline(always)]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn get(&self) -> &T {
        // SAFETY: `current` names the bank owned for shared reads by this
        // endpoint. Activation needs `&mut self`, so it cannot change while
        // the returned borrow is live.
        unsafe { &*self.buf.banks[self.current as usize].get() }
    }

    /// Activate a committed bank, ignoring invalid or foreign tokens.
    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn activate(&mut self, token: CommitToken) {
        if token.owner != self.buf.identity() {
            return;
        }
        let pending = self.buf.pending.load(Ordering::Acquire);
        if pending == NO_PENDING || pending - 1 != token.bank {
            return;
        }
        self.current = pending - 1;
        self.buf.active.store(self.current, Ordering::Release);
        // Release pairs with the staging endpoint's next Acquire loads, which
        // must observe the new active bank before selecting the inactive one.
        self.buf.pending.store(NO_PENDING, Ordering::Release);
    }
}

/// A commit token cannot be duplicated for replay.
///
/// ```compile_fail
/// use helic_core::TableBuffer;
/// use static_cell::ConstStaticCell;
///
/// static BUFFER: ConstStaticCell<TableBuffer> =
///     ConstStaticCell::new(TableBuffer::new());
/// let (mut staging, _) = BUFFER.take().split();
/// let token = staging.commit().unwrap();
/// let moved = token;
/// let duplicated = token;
/// drop((moved, duplicated));
/// ```
const _: () = ();

#[cfg(test)]
mod tests {
    use std::boxed::Box;

    use super::*;

    fn endpoints() -> (Staging, Active) {
        Box::leak(Box::new(TableBuffer::new())).split()
    }

    fn stage(staging: &mut Staging, values: &[f32]) {
        let table = staging.buffer().unwrap();
        assert!(table.write_block(0, values));
        assert!(table.set_len(values.len()));
    }

    #[test]
    fn pending_commit_is_busy_and_cancel_restores_writability() {
        let (mut staging, _active) = endpoints();
        stage(&mut staging, &[1.0, 2.0]);
        let token = staging.commit().unwrap();
        assert_eq!(staging.commit().unwrap_err(), BufferError::Busy);
        assert!(matches!(staging.buffer(), Err(BufferError::Busy)));
        staging.cancel(token);
        assert!(staging.buffer().is_ok());
    }

    #[test]
    fn activation_observes_all_writes_published_by_commit() {
        let (mut staging, mut active) = endpoints();
        let values = [1.0, -2.5, 3.25, 4.0];
        stage(&mut staging, &values);
        active.activate(staging.commit().unwrap());
        assert_eq!(active.get().values(), values);
    }

    #[test]
    fn value_buffer_uses_the_same_publication_protocol() {
        let (mut staging, mut active) = Box::leak(Box::new(ValueBuffer::<132>::values())).split();
        staging.buffer().unwrap()[..4].copy_from_slice(&[1.0, -2.5, 3.25, 4.0]);
        active.activate(staging.commit().unwrap());
        assert_eq!(&active.get()[..4], &[1.0, -2.5, 3.25, 4.0]);
    }

    #[test]
    fn rejected_commit_leaves_active_bank_untouched() {
        let (mut staging, mut active) = endpoints();
        stage(&mut staging, &[1.0, 2.0]);
        active.activate(staging.commit().unwrap());

        stage(&mut staging, &[7.0, 8.0, 9.0]);
        let rejected = staging.commit().unwrap();
        staging.cancel(rejected);

        assert_eq!(active.get().values(), [1.0, 2.0]);
        assert!(staging.buffer().is_ok());
    }

    #[test]
    fn foreign_tokens_cannot_cancel_or_activate_any_buffer_type() {
        let (mut staging_a, _active_a) = endpoints();
        let (mut staging_b, mut active_b) = endpoints();
        stage(&mut staging_a, &[1.0, 2.0]);
        stage(&mut staging_b, &[3.0, 4.0]);
        let token_a = staging_a.commit().unwrap();
        let token_b = staging_b.commit().unwrap();

        staging_b.cancel(token_a);
        assert!(matches!(staging_b.buffer(), Err(BufferError::Busy)));
        active_b.activate(token_b);
        assert_eq!(active_b.get().values(), [3.0, 4.0]);

        let (mut table_staging, _table_active) = endpoints();
        let (_values_staging, mut values_active) =
            Box::leak(Box::new(ValueBuffer::<4>::values())).split();
        stage(&mut table_staging, &[5.0, 6.0]);
        values_active.activate(table_staging.commit().unwrap());
        assert_eq!(*values_active.get(), [0.0; 4]);
    }

    #[test]
    fn commit_activation_cycles_do_not_exhaust() {
        let (mut staging, mut active) = endpoints();
        for value in 0..100_000_u32 {
            stage(&mut staging, &[value as f32, value.wrapping_add(1) as f32]);
            active.activate(staging.commit().unwrap());
            assert_eq!(active.get().values()[0], value as f32);
        }
    }
}
