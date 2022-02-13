#![allow(dead_code)]

use std::borrow::Cow;
use std::ffi::{CStr, CString};
use std::fmt::{Debug, Display, Formatter};
use std::hash::{Hash, Hasher};
use std::lazy::SyncLazy;
use std::ops::Deref;
use std::ptr::NonNull;

use bumpalo::Bump;
use parking_lot::Mutex;

pub use super::uhash::{ahash, IdentityHashMap};

const BUCKET_COUNT: u64 = 64;
const BUCKET_BASE_CAPACITY: usize = 0;

#[repr(C)]
struct UniqueStringEntry {
    length: usize,
    hash: u64,
    string: NonNull<u8>,
}

struct UniqueStringBucket {
    alloc: Bump,
    store: IdentityHashMap<u64, NonNull<u8>>,
}
#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl Send for UniqueStringBucket {}
impl UniqueStringBucket {
    #[inline]
    fn new() -> Self {
        Self {
            alloc: Bump::with_capacity(
                BUCKET_BASE_CAPACITY * std::mem::size_of::<UniqueStringEntry>() * 4,
            ),
            store: IdentityHashMap::<u64, NonNull<u8>>::with_capacity_and_hasher(
                BUCKET_BASE_CAPACITY,
                Default::default(),
            ),
        }
    }

    fn store(&mut self, string: &str, hash: u64) -> NonNull<u8> {
        let str_addr = self
            .alloc
            .alloc_slice_copy(&[string.as_bytes(), &[0]].concat());
        let str_addr = unsafe { NonNull::new_unchecked(str_addr as *const _ as *mut _) };
        let ent_addr = self.alloc.alloc(UniqueStringEntry {
            length: string.len(),
            hash,
            string: str_addr,
        });
        let ent_addr = unsafe { NonNull::new_unchecked(ent_addr as *const _ as *mut _) };
        self.store.insert(hash, ent_addr);
        ent_addr
    }

    #[inline]
    fn get(&self, hash: u64) -> Option<NonNull<u8>> {
        self.store.get(&hash).copied()
    }
}

struct UniqueStringStore {
    buckets: [Mutex<UniqueStringBucket>; BUCKET_COUNT as usize],
}
impl UniqueStringStore {
    fn new() -> Self {
        Self {
            buckets: [(); BUCKET_COUNT as usize].map(|_| Mutex::new(UniqueStringBucket::new())),
        }
    }

    #[inline]
    fn get_or_store(&self, string: &str, hash: u64) -> NonNull<u8> {
        let mut store = self.buckets[(hash % BUCKET_COUNT) as usize].lock();
        store.get(hash).unwrap_or_else(|| store.store(string, hash))
    }
}

static INTERNED_STRINGS: SyncLazy<UniqueStringStore> = SyncLazy::new(UniqueStringStore::new);

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct UniqueString {
    entry: NonNull<u8>,
}
unsafe impl Send for UniqueString {}
unsafe impl Sync for UniqueString {}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct UniqueStringIntermediary<'a> {
    hash: u64,
    string: &'a str,
}

impl UniqueString {
    #[inline]
    pub fn new(string: &str) -> Self {
        Self::create(string).intern()
    }

    #[inline]
    pub const fn create(string: &str) -> UniqueStringIntermediary<'_> {
        UniqueStringIntermediary {
            hash: ahash(string.as_bytes()),
            string,
        }
    }

    #[inline]
    fn new_with_hash(string: &str, hash: u64) -> Self {
        Self::create_with_hash(string, hash).intern()
    }

    #[inline]
    const fn create_with_hash(string: &str, hash: u64) -> UniqueStringIntermediary<'_> {
        UniqueStringIntermediary { hash, string }
    }

    #[inline]
    pub fn as_str(&self) -> &'static str {
        unsafe {
            let entry = (self.entry.as_ptr() as *const UniqueStringEntry)
                .as_ref()
                .unwrap();
            let slice = std::slice::from_raw_parts(entry.string.as_ptr(), entry.length);
            std::str::from_utf8_unchecked(slice)
        }
    }

    #[inline]
    pub fn as_cstr(&self) -> &'static CStr {
        unsafe {
            let entry = (self.entry.as_ptr() as *const UniqueStringEntry)
                .as_ref()
                .unwrap();
            std::ffi::CStr::from_bytes_with_nul_unchecked(std::slice::from_raw_parts(
                entry.string.as_ptr(),
                entry.length + 1,
            ))
        }
    }

    #[inline]
    pub fn as_cow(&self) -> Cow<'static, str> {
        Cow::Borrowed(self.as_str())
    }

    #[inline]
    pub fn hash(&self) -> u64 {
        unsafe {
            (self.entry.as_ptr() as *const UniqueStringEntry)
                .as_ref()
                .unwrap()
                .hash
        }
    }

    #[inline]
    pub fn entry(&self) -> NonNull<u8> {
        self.entry
    }
}
impl UniqueStringIntermediary<'_> {
    #[inline]
    pub fn intern(self) -> UniqueString {
        UniqueString {
            entry: INTERNED_STRINGS.get_or_store(self.string, self.hash),
        }
    }
}
impl From<UniqueStringIntermediary<'_>> for UniqueString {
    #[inline]
    fn from(intermediary: UniqueStringIntermediary<'_>) -> Self {
        intermediary.intern()
    }
}
impl From<UniqueString> for &str {
    #[inline]
    fn from(string: UniqueString) -> Self {
        string.as_str()
    }
}
impl From<&str> for UniqueString {
    #[inline]
    fn from(string: &str) -> Self {
        Self::new(string)
    }
}
impl From<UniqueString> for String {
    #[inline]
    fn from(string: UniqueString) -> Self {
        string.as_str().into()
    }
}
impl From<String> for UniqueString {
    #[inline]
    fn from(string: String) -> Self {
        Self::new(string.as_str())
    }
}
impl From<&String> for UniqueString {
    #[inline]
    fn from(string: &String) -> Self {
        Self::new(string.as_str())
    }
}
impl From<UniqueString> for &CStr {
    #[inline]
    fn from(string: UniqueString) -> Self {
        string.as_cstr()
    }
}
impl From<UniqueString> for CString {
    #[inline]
    fn from(string: UniqueString) -> Self {
        string.as_cstr().into()
    }
}
impl From<UniqueString> for Cow<'_, str> {
    #[inline]
    fn from(string: UniqueString) -> Self {
        string.as_cow()
    }
}
impl Deref for UniqueString {
    type Target = str;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}
impl AsRef<str> for UniqueString {
    #[inline]
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}
impl AsRef<CStr> for UniqueString {
    #[inline]
    fn as_ref(&self) -> &CStr {
        self.as_cstr()
    }
}

#[allow(clippy::derive_hash_xor_eq)]
impl Hash for UniqueString {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.hash().hash(state)
    }
}
impl Display for UniqueString {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
impl Debug for UniqueString {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if f.alternate() {
            write!(
                f,
                "UniqueString {{\n    string: {:?},\n    hash: {:?},\n    entry: {:?}\n}}",
                self.as_str(),
                self.hash(),
                self.entry
            )
        } else {
            write!(
                f,
                "UniqueString {{ string: {:?}, hash: {:?}, entry: {:?} }}",
                self.as_str(),
                self.hash(),
                self.entry
            )
        }
    }
}
