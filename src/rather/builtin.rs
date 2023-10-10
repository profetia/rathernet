//! # Rathernet Ather
//! Rathernet ather are used to send and receive data in bits. The data is encoded in the form of
//! audio signals in the method of phase shift keying (PSK). The stream is composed of a preamble
//! (PREAMBLE_SYMBOL_LEN symbols), a length (LENGTH_BITS_LEN symbols) and a payload (PAYLOAD_BITS_LEN
//! symbols with maximum 1 << LENGTH_BITS_LEN - 1 symbols). The preamble is used to identify the
//! start of a frame. The length is used to indicate the length of the payload.

pub const WARMUP_SYMBOL_LEN: usize = 0;
pub const PREAMBLE_SYMBOL_LEN: usize = 64; // 8 | 16 | 32 | 64 | 112 | 224
pub const PREAMBLE_CORR_THRESHOLD: f32 = 0.4;

pub const LENGTH_BITS_LEN: usize = 10; // 6 | 7 | 8 | 9 | 10
pub const PAYLOAD_BITS_LEN: usize = (1 << LENGTH_BITS_LEN) - 1;
