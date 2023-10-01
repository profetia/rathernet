//! # Rathernet CSMA/CA
//! Rathernet CSMA/CA is used to handle multiple access to the medium. The medium is a shared
//! channel available to multiple nodes. The nodes are not synchronized and can send data at any
//! time. The nodes listen to the medium and wait for a certain amount of time before sending data.
//! If the medium is busy, the node waits for a random amount of time before trying again.
//! Otherwise, the node sends the data and waits for an ACK. If the ACK is not received, the node
//! waits for a random amount of time before trying again.
//! ## Frame structure
//! Ather: the frame structure of ather is the actual frame structure transmitted in the medium.
//! | Preamble (PREAMBLE_SYMBOL_LEN bits) | Length (LENGTH_BITS_LEN) | Payload (<= ATHER_PAYLOAD_BITS_LEN) |
//! CSMA/CA: the frame structure of CSMA/CA resides in the payload of ather frames.
//! | Dest (ADDRESS_BITS_LEN) | Src (ADDRESS_BITS_LEN) | Seq (SEQ_BITS_LEN) | Type (TYPE_BITS_LEN) |
//! | Payload (<= PAYLOAD_BITS_LEN) | Parity (PARITY_BITS_LEN) |

use crate::rather::builtin::PAYLOAD_BITS_LEN as ATHER_PAYLOAD_BITS_LEN;
use crc::{Crc, CRC_16_IBM_SDLC};
use std::time::Duration;

pub const ADDRESS_BITS_LEN: usize = 4;
pub const SEQ_BITS_LEN: usize = 8;
pub const TYPE_BITS_LEN: usize = 4;

pub const PARITY_ALGORITHM: Crc<u16> = Crc::<u16>::new(&CRC_16_IBM_SDLC);
pub const PARITY_BITS_LEN: usize = 16;

pub const PAYLOAD_BITS_LEN: usize = ATHER_PAYLOAD_BITS_LEN
    - ADDRESS_BITS_LEN
    - ADDRESS_BITS_LEN
    - TYPE_BITS_LEN
    - SEQ_BITS_LEN
    - PARITY_BITS_LEN;

pub const FRAME_DETECT_TIMEOUT: Duration = Duration::from_millis(1000);

pub const ACK_LINK_TIMEOUT: Duration = Duration::from_millis(10000);
pub const ACK_RECIEVE_TIMEOUT: Duration = Duration::from_millis(3000);
pub const ACK_WINDOW_SIZE: usize = 16; // 8 | 16 | 32 | 64
