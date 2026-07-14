//! Transport-independent host protocol servers and stream session state.

pub mod beacon;
pub mod tcp;
pub mod udp;

use core::cell::RefCell;

use embassy_net::Ipv4Address;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex;
use heapless::Vec;

use crate::rig::MAX_SOURCES;

/// Stream session state shared between the TCP server (writer) and the UDP
/// streamer (reader). Both tasks live on core 0.
pub struct StreamState {
    /// Stream target; `None` until a `StreamStart` arrives.
    pub target: Option<(Ipv4Address, u16)>,
    pub enabled: bool,
    /// Source ids in the experiment's discovered source-table order.
    pub sources: Vec<u8, MAX_SOURCES>,
    /// Keep every n-th sample (>= 1).
    pub decimation: u16,
    /// Records to send before auto-stop; 0 = continuous.
    pub count: u32,
    /// Incremented by every `StreamStart`; the streamer re-arms on change.
    pub generation: u32,
}

pub static STREAM: Mutex<CriticalSectionRawMutex, RefCell<StreamState>> =
    Mutex::new(RefCell::new(StreamState {
        target: None,
        enabled: false,
        sources: Vec::new(),
        decimation: 1,
        count: 0,
        generation: 0,
    }));
