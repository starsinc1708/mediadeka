mod fog;
mod motion;

pub(super) use fog::{Transfer, transfer_fog};
pub(super) use motion::{Motion, MotionFrame};

pub(super) const ACCENTS: [u32; 5] = [0x1ed760, 0xff2537, 0xffcc27, 0xff742a, 0x8c83ff];
pub(super) const CONNECTION_RADIUS: f32 = 12.;
