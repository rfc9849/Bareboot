#[cfg(target_os = "linux")]
pub mod ahb_wire;
pub mod codec;
pub mod fdpass;

pub use codec::{
    CheckState, DaemonOptions, DistroStatus, ImageInfo, KernelCheck, KernelInfo, PortForward, Request, Response,
    SessionInfo, WinSize, MAX_FRAME_SIZE, decode_request, decode_response, encode_request, encode_response,
};
pub use fdpass::{recv_frame, send_frame};

pub const PROTOCOL_VERSION: u32 = 19;
pub const DEFAULT_SOCKET_PATH: &str = "/dev/socket/bareboot/barebootd.sock";
pub const DEFAULT_STATE_DIR: &str = "/data/adb/bareboot";
pub const DEFAULT_SHARED_DIR: &str = "/data/media/0/Linux";
