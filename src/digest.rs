//! SHA-256 of a file, for the blind comparison.
//!
//! Two encodes of one song carry the same tags, so the tags cannot say which
//! file is which; the sums can. A long file takes long enough to be felt in a
//! window, so both are read on a thread of their own and arrive over a
//! channel.
//!
//! The hash is written out here rather than pulled in, since it is one
//! function over a table of constants, and the app ships no dependency it
//! does not need.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::thread;

/// How much of a file is read at a time.
const CHUNK: usize = 64 * 1024;

/// The first thirty-two bits of the fractional parts of the square roots of
/// the first eight primes, which is where the digest starts.
const INITIAL: [u32; 8] = [
    0x6a09_e667,
    0xbb67_ae85,
    0x3c6e_f372,
    0xa54f_f53a,
    0x510e_527f,
    0x9b05_688c,
    0x1f83_d9ab,
    0x5be0_cd19,
];

/// The first thirty-two bits of the fractional parts of the cube roots of the
/// first sixty-four primes, one for each round.
const K: [u32; 64] = [
    0x428a_2f98,
    0x7137_4491,
    0xb5c0_fbcf,
    0xe9b5_dba5,
    0x3956_c25b,
    0x59f1_11f1,
    0x923f_82a4,
    0xab1c_5ed5,
    0xd807_aa98,
    0x1283_5b01,
    0x2431_85be,
    0x550c_7dc3,
    0x72be_5d74,
    0x80de_b1fe,
    0x9bdc_06a7,
    0xc19b_f174,
    0xe49b_69c1,
    0xefbe_4786,
    0x0fc1_9dc6,
    0x240c_a1cc,
    0x2de9_2c6f,
    0x4a74_84aa,
    0x5cb0_a9dc,
    0x76f9_88da,
    0x983e_5152,
    0xa831_c66d,
    0xb003_27c8,
    0xbf59_7fc7,
    0xc6e0_0bf3,
    0xd5a7_9147,
    0x06ca_6351,
    0x1429_2967,
    0x27b7_0a85,
    0x2e1b_2138,
    0x4d2c_6dfc,
    0x5338_0d13,
    0x650a_7354,
    0x766a_0abb,
    0x81c2_c92e,
    0x9272_2c85,
    0xa2bf_e8a1,
    0xa81a_664b,
    0xc24b_8b70,
    0xc76c_51a3,
    0xd192_e819,
    0xd699_0624,
    0xf40e_3585,
    0x106a_a070,
    0x19a4_c116,
    0x1e37_6c08,
    0x2748_774c,
    0x34b0_bcb5,
    0x391c_0cb3,
    0x4ed8_aa4a,
    0x5b9c_ca4f,
    0x682e_6ff3,
    0x748f_82ee,
    0x78a5_636f,
    0x84c8_7814,
    0x8cc7_0208,
    0x90be_fffa,
    0xa450_6ceb,
    0xbef9_a3f7,
    0xc671_78f2,
];

/// The two sums a comparison shows, read on a thread of their own.
///
/// They are read together because a test always has both, and the window has
/// nothing to show until it has them.
pub type Sums = (Result<String, String>, Result<String, String>);

/// Starts reading the sums of `a` and `b`, and returns the channel they
/// arrive on.
pub fn spawn(a: PathBuf, b: PathBuf) -> Receiver<Sums> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send((sum(&a), sum(&b)));
    });
    receiver
}

/// The SHA-256 of `bytes`, as lower-case hex.
///
/// The comparison signs its log with this, so a reader can hash the log again
/// and see the same sum.
pub fn hex(bytes: &[u8]) -> String {
    let mut sha = Sha256::new();
    sha.update(bytes);
    sha.finish()
}

/// The SHA-256 of a file, as lower-case hex.
///
/// The file is read a chunk at a time, so a long one costs no more memory
/// than a short one.
pub fn sum(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut sha = Sha256::new();
    let mut chunk = vec![0_u8; CHUNK];
    loop {
        let read = file.read(&mut chunk).map_err(|error| error.to_string())?;
        if read == 0 {
            return Ok(sha.finish());
        }
        sha.update(&chunk[..read]);
    }
}

/// A SHA-256 that takes the bytes as they come.
struct Sha256 {
    /// The eight words the digest is built in.
    state: [u32; 8],
    /// The bytes that do not fill a block yet.
    buffer: [u8; 64],
    /// How much of `buffer` is filled.
    filled: usize,
    /// Bytes taken in so far, which the length padding needs.
    length: u64,
}

impl Sha256 {
    /// Starts a digest.
    fn new() -> Self {
        Self {
            state: INITIAL,
            buffer: [0; 64],
            filled: 0,
            length: 0,
        }
    }

    /// Takes in `bytes`, compressing every block they complete.
    fn update(&mut self, mut bytes: &[u8]) {
        self.length = self.length.wrapping_add(bytes.len() as u64);
        if self.filled > 0 {
            let take = (64 - self.filled).min(bytes.len());
            self.buffer[self.filled..self.filled + take].copy_from_slice(&bytes[..take]);
            self.filled += take;
            bytes = &bytes[take..];
            if self.filled == 64 {
                let block = self.buffer;
                self.compress(&block);
                self.filled = 0;
            }
        }
        while bytes.len() >= 64 {
            let (block, rest) = bytes.split_at(64);
            let block: &[u8; 64] = block.try_into().expect("a block is sixty-four bytes");
            self.compress(block);
            bytes = rest;
        }
        if !bytes.is_empty() {
            self.buffer[..bytes.len()].copy_from_slice(bytes);
            self.filled = bytes.len();
        }
    }

    /// Pads the message and returns the digest as lower-case hex.
    fn finish(mut self) -> String {
        // A one bit, then zeros, then the length in bits, so that the last
        // block ends exactly sixty-four bits short.
        let bits = self.length.wrapping_mul(8);
        let zeros = if self.filled < 56 {
            55 - self.filled
        } else {
            119 - self.filled
        };
        let mut padding = Vec::with_capacity(1 + zeros + 8);
        padding.push(0x80);
        padding.resize(1 + zeros, 0);
        padding.extend_from_slice(&bits.to_be_bytes());
        self.update(&padding);
        self.state
            .iter()
            .map(|word| format!("{word:08x}"))
            .collect()
    }

    /// Folds one sixty-four byte block into the state.
    fn compress(&mut self, block: &[u8; 64]) {
        let mut schedule = [0_u32; 64];
        let (words, _) = block.as_chunks::<4>();
        for (index, word) in words.iter().enumerate() {
            schedule[index] = u32::from_be_bytes(*word);
        }
        for index in 16..64 {
            let first = schedule[index - 15];
            let second = schedule[index - 2];
            let s0 = first.rotate_right(7) ^ first.rotate_right(18) ^ (first >> 3);
            let s1 = second.rotate_right(17) ^ second.rotate_right(19) ^ (second >> 10);
            schedule[index] = schedule[index - 16]
                .wrapping_add(s0)
                .wrapping_add(schedule[index - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for (index, round) in K.iter().enumerate() {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choice = (e & f) ^ (!e & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(choice)
                .wrapping_add(*round)
                .wrapping_add(schedule[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(majority);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }

        for (word, add) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *word = word.wrapping_add(add);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of this test's own, removed when it drops.
    struct Temporary(PathBuf);

    impl Temporary {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("blue-dove-digest-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("the temporary directory is created");
            Self(path)
        }
    }

    impl Drop for Temporary {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_digest_matches_the_published_vectors() {
        // The two everyone quotes, and the millionth `a`, which crosses many
        // blocks and so catches a padding or block-chaining mistake.
        let directory = Temporary::new("vectors");
        let cases = [
            (
                "empty",
                Vec::new(),
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            ),
            (
                "abc",
                b"abc".to_vec(),
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
            (
                "million",
                vec![b'a'; 1_000_000],
                "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0",
            ),
        ];
        for (name, bytes, expected) in cases {
            let path = directory.0.join(name);
            std::fs::write(&path, &bytes).expect("the file is written");
            assert_eq!(sum(&path).expect("the file is read"), expected, "{name}");
        }
    }

    #[test]
    fn a_file_read_in_chunks_hashes_as_one_read_whole() {
        // The file is read a chunk at a time, so a block boundary that falls
        // inside a chunk has to be handled the same way as one that does not.
        let bytes: Vec<u8> = (0..(3 * CHUNK + 17)).map(|index| index as u8).collect();
        let mut whole = Sha256::new();
        whole.update(&bytes);
        let expected = whole.finish();

        let mut chunked = Sha256::new();
        for piece in bytes.chunks(CHUNK) {
            chunked.update(piece);
        }
        assert_eq!(chunked.finish(), expected);
    }

    #[test]
    fn bytes_are_hashed_the_same_way_as_the_file_holding_them() {
        let directory = Temporary::new("bytes");
        let path = directory.0.join("abc");
        std::fs::write(&path, b"abc").expect("the file is written");
        assert_eq!(hex(b"abc"), sum(&path).expect("the file is read"));
    }

    #[test]
    fn a_file_that_is_not_there_is_an_error() {
        let directory = Temporary::new("missing");
        assert!(sum(&directory.0.join("gone.flac")).is_err());
    }
}
