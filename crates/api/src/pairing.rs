//! Pairing a device with this PC (`/api/pair/*`, `/api/devices`).

use serde::{Deserialize, Serialize};

/// `GET /api/pair/me`: whether this device may use Portal, and whether it is the PC itself.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairMe {
    pub paired: bool,
    pub this_pc: bool,
}

/// `POST /api/pair/start`: the number to type on the PC, and the ticket to poll with.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairStarted {
    pub code: String,
    pub ticket: String,
    pub expires_in: u64,
}

/// Where a pairing stands (`GET /api/pair/status?ticket=`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PairState {
    Waiting,
    Paired,
    Expired,
}

/// `GET /api/pair/status?ticket=`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairStatus {
    pub state: PairState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TicketQuery {
    pub ticket: String,
}

/// `POST /api/pair/approve`, sent from the PC with the number the device shows.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairApprove {
    pub code: String,
}

/// The answer to `POST /api/pair/approve`: the device now paired.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairApproved {
    // In the order the server's json! map wrote them (alphabetical).
    pub name: String,
    pub ok: bool,
}

/// One row of `GET /api/devices`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairedDevice {
    pub id: String,
    pub name: String,
    pub paired_at: i64,
    pub last_seen: i64,
    pub address: String,
}
