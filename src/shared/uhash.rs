#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};

use byteorder::{ByteOrder, NativeEndian};

#[derive(Default, Clone, Copy)]
pub struct IdentityHasher {
    hash: u64,
}
impl Hasher for IdentityHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        if bytes.len() == 8 {
            self.hash = self.hash.wrapping_add(NativeEndian::read_u64(bytes));
        } else {
            unreachable!()
        }
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }
}

pub type IdentityHashMap<K, V> = HashMap<K, V, BuildHasherDefault<IdentityHasher>>;
pub type IdentityHashSet<V> = HashSet<V, BuildHasherDefault<IdentityHasher>>;

#[inline(always)]
const fn folded_multiply(s: u64, by: u64) -> u64 {
    let result = (s as u128).wrapping_mul(by as u128);
    ((result & 0xffff_ffff_ffff_ffff) as u64) ^ ((result >> 64) as u64)
}
pub const fn ahash(data: &[u8]) -> u64 {
    const MULTIPLE: u64 = 6364136223846793005u64;
    const PAD: u64 = 238757555275374294u64;

    let mut buffer: u64 = 7387682673934926105u64;
    let mut rindex: usize = 0;
    let mut dindex: usize = 0;
    let mut udata: u64;

    let limit = data.len() / 8 + 1;
    loop {
        if rindex >= limit {
            break;
        }
        udata = 0;
        loop {
            if dindex >= data.len() {
                break;
            }
            udata |= (data[dindex] as u64) << ((dindex % 8) * 8);
            dindex += 1;
            if dindex % 8 == 0 {
                break;
            }
        }
        buffer = folded_multiply(udata ^ buffer, MULTIPLE);
        rindex += 1;
    }
    (buffer.wrapping_mul(MULTIPLE) ^ PAD).rotate_left((buffer & 63) as u32)
}
